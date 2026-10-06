//! Studio-originated preset mutations: what Apply, Reapply, Reset and a project override
//! edit write into a project, decided purely from the current source.
//!
//! The controller ([`crate::Controller::apply_preset`]) owns writing: it turns the
//! [`PresetPlan`] built here into a candidate file set and publishes it through the same
//! durable, no-follow, no-clobber transaction and recovery protocol as an accepted agent
//! edit ([`crate::edit_transaction`]). The committed record is a distinct
//! [`crate::TransactionKind::Preset`] entry carrying a [`PresetProvenance`]; it is never
//! accepted task history, so Undo and the agent history cannot see it.
//!
//! # Ownership of project files
//!
//! | Project path | Owner |
//! |---|---|
//! | `style/tokens.json`, `style/preset.json`, `style/preset-tokens.json`, `style/guide.md` | the preset snapshot (rewritten on every application) |
//! | `style/overrides.json` | the user: preserved **byte-for-byte** by Apply/Reapply while it still reads against the new preset; cleared only by an explicit Reset |
//! | `style/preset/**`, `media/preset-*` | the preset snapshot (licenses, examples; fonts and images as flat top-level `media/` files, because the renderer reads only those); the files named by `style/preset.json`'s `media` list are replaced as a set when the preset changes, other `media/` files are never touched |
//! | `studio.json` `preset` | updated to the applied `{id, package hash}` *after* the file set commits, by the project's same-directory atomic manifest replacement: the manifest must never be displaced by a transaction, or a crash in that window would leave a project that cannot be opened (and so cannot recover). A crash between the two leaves a stale reference that is repaired on the next open or mutation |
//!
//! An override that no longer applies (the new preset has no such token, or a different
//! type) is never deleted: the file is kept, resolution leaves the entry out of the
//! snapshot and the entry is surfaced in [`PresetProvenance::notes`].
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
use studio_presets::{
    OverridesFile, Package, TokenDef, TokenName,
    model::{Design, parse_literal},
    project::{
        MEDIA_PREFIX, NOTICE_PREFIX, OVERRIDES_PATH, PRESET_PATH, PRESET_TOKENS_PATH,
        PresetIdentity, TOKENS_PATH,
    },
    reresolve_project_style, resolve,
};
use studio_project::{Manifest, ProjectPath, SourceInventory, manifest::PresetReference};

/// Largest style file read back from a project (they are all small JSON/text).
const MAX_STYLE_FILE: u64 = 8 * 1024 * 1024;

/// Whether this build may publish preset mutations. Only the platform whose no-clobber
/// publication and crash-recovery gates were measured is enabled; every other
/// OS/architecture combination stays disabled until its own probes pass.
pub const MUTATION_QUALIFIED: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

/// What a preset mutation did, recorded durably in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetAction {
    /// Choose a preset (the first one, or a different one); overrides are merged.
    Apply,
    /// Refresh the same preset; overrides are merged.
    Reapply,
    /// Apply the preset and clear every project override (explicit).
    Reset,
    SetOverride,
    ClearOverride,
}

impl PresetAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Apply => "Apply",
            Self::Reapply => "Reapply",
            Self::Reset => "Reset",
            Self::SetOverride => "Set override",
            Self::ClearOverride => "Clear override",
        }
    }
}

/// Provenance of a committed preset mutation; independent of agent task history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetProvenance {
    pub action: PresetAction,
    pub preset_id: String,
    /// Package hash, also the manifest `PresetReference::sha256`.
    pub package_sha256: String,
    /// Hash of the resulting runtime snapshot (`style/tokens.json`).
    pub tokens_sha256: String,
    /// The preset that was applied before, if any.
    pub previous_preset: Option<String>,
    /// Overrides that were kept in the file but not applied, and other findings.
    pub notes: Vec<String>,
}

/// Why a preset mutation could not be planned.
#[derive(Debug, thiserror::Error)]
pub enum PresetStateError {
    #[error("no preset is applied to this project yet; choose a preset first")]
    NoPreset,
    #[error("Reapply needs the preset that is applied here ({applied}), not {requested}")]
    WrongPreset { applied: String, requested: String },
    #[error("{0}")]
    Preset(#[from] studio_presets::PresetError),
    #[error("{0}")]
    Diagnostic(String),
    #[error("{0}")]
    Project(#[from] studio_project::ProjectError),
    #[error("{what}: {reason}")]
    Style { what: String, reason: String },
    #[error("the override is not applied by the preset: {0}")]
    NotApplied(String),
    #[error("`{0}` is not a project override")]
    NotAnOverride(String),
}

impl From<studio_presets::Diagnostic> for PresetStateError {
    fn from(value: studio_presets::Diagnostic) -> Self {
        Self::Diagnostic(value.to_string())
    }
}

fn style(what: &str, reason: impl ToString) -> PresetStateError {
    PresetStateError::Style {
        what: what.to_owned(),
        reason: reason.to_string(),
    }
}

/// What the user asked for.
#[derive(Debug, Clone)]
pub enum PresetRequest<'a> {
    /// `action` is `Apply`, `Reapply` or `Reset`.
    Select {
        package: &'a Package,
        action: PresetAction,
    },
    /// Replace one project override with an authored literal (`#rrggbb`, a number of
    /// pixels or a length string, a duration, a typography object, ...).
    SetOverride {
        name: &'a str,
        value: &'a serde_json::Value,
    },
    ClearOverride {
        name: &'a str,
    },
}

/// One path of the after file set. `bytes == None` deletes the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: ProjectPath,
    pub bytes: Option<Vec<u8>>,
}

/// The complete change a preset mutation makes to the source.
#[derive(Debug, Clone)]
pub struct PresetPlan {
    pub provenance: PresetProvenance,
    pub summary: String,
    /// The `{id, package hash}` the manifest must reference once the file set commits.
    pub reference: Option<(String, String)>,
    /// Only paths whose bytes differ from the current source.
    pub changes: Vec<FileChange>,
}

/// A project's current style snapshot as stored in its portable files.
#[derive(Debug, Clone)]
pub struct ProjectStyle {
    pub identity: PresetIdentity,
    /// Overrides that replaced a preset default: `(token, layer, value as JSON)`.
    pub overridden: Vec<(String, String, String)>,
    /// Entries kept in `style/overrides.json` that the snapshot does not apply.
    pub diagnostics: Vec<String>,
    /// SHA-256 of `style/tokens.json` as it is on disk.
    pub tokens_sha256: Option<String>,
}

/// Reads one source file of the inventory, verifying the bytes are the inventoried ones.
fn read_source(
    root: &Path,
    inventory: &SourceInventory,
    path: &str,
) -> Result<Option<Vec<u8>>, PresetStateError> {
    let Some(file) = inventory.files.iter().find(|f| f.path.as_str() == path) else {
        return Ok(None);
    };
    if file.size > MAX_STYLE_FILE {
        return Err(style(path, "file is too large to be a style file"));
    }
    let mut bytes = Vec::with_capacity(file.size as usize);
    file.path
        .open_file(root)?
        .take(MAX_STYLE_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| style(path, e))?;
    if format!("{:x}", Sha256::digest(&bytes)) != file.sha256 {
        return Err(style(path, "changed while it was being read"));
    }
    Ok(Some(bytes))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn project_path(path: &str) -> ProjectPath {
    ProjectPath::try_from(path.to_owned()).expect("constant style paths are valid")
}

fn current_identity(
    root: &Path,
    inventory: &SourceInventory,
) -> Result<Option<PresetIdentity>, PresetStateError> {
    Ok(read_source(root, inventory, PRESET_PATH)?
        .and_then(|bytes| serde_json::from_slice::<PresetIdentity>(&bytes).ok()))
}

/// The project's applied style, if it has a readable preset snapshot.
pub fn read_project_style(root: &Path, inventory: &SourceInventory) -> Option<ProjectStyle> {
    let preset = read_source(root, inventory, PRESET_PATH).ok()??;
    let tokens = read_source(root, inventory, PRESET_TOKENS_PATH).ok()??;
    let overrides = match read_source(root, inventory, OVERRIDES_PATH).ok()? {
        Some(bytes) => bytes,
        None => OverridesFile::default().to_canonical_bytes(),
    };
    let (identity, snapshot) = reresolve_project_style(&preset, &tokens, &overrides, None).ok()?;
    let overridden = snapshot
        .overridden_by()
        .iter()
        .map(|(name, layer)| {
            let value = snapshot
                .get(name.as_str())
                .and_then(|v| serde_json::to_string(v).ok())
                .unwrap_or_default();
            (
                name.to_string(),
                format!("{layer:?}").to_ascii_lowercase(),
                value,
            )
        })
        .collect();
    let diagnostics = snapshot
        .diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect();
    let tokens_sha256 = inventory
        .files
        .iter()
        .find(|f| f.path.as_str() == TOKENS_PATH)
        .map(|f| f.sha256.clone());
    Some(ProjectStyle {
        identity,
        overridden,
        diagnostics,
        tokens_sha256,
    })
}

/// Identity of the active style snapshot for a task context: the preset, its package hash
/// and the hash of the resolved tokens actually on disk.
pub fn style_identity(
    root: &Path,
    inventory: &SourceInventory,
) -> Option<crate::StyleSnapshotIdentity> {
    let style = read_project_style(root, inventory)?;
    Some(crate::StyleSnapshotIdentity {
        preset_id: style.identity.id,
        preset_hash: style.identity.hash,
        resolved_tokens_hash: style.tokens_sha256?,
    })
}

fn differs(inventory: &SourceInventory, path: &ProjectPath, bytes: &[u8]) -> bool {
    inventory
        .files
        .iter()
        .find(|f| &f.path == path)
        .is_none_or(|f| f.executable || f.sha256 != sha256(bytes))
}

/// `manifest` carrying the `{id, package hash}` reference, or `None` when it already does.
pub fn manifest_with_reference(manifest: &Manifest, id: &str, hash: &str) -> Option<Manifest> {
    let wanted = PresetReference {
        id: id.to_owned(),
        sha256: hash.to_owned(),
    };
    (manifest.preset.as_ref() != Some(&wanted)).then(|| Manifest {
        preset: Some(wanted),
        ..manifest.clone()
    })
}

/// Fails unless `studio.json` on disk is exactly the inventoried bytes: the fence checked
/// immediately before the manifest is replaced.
pub fn verify_manifest_unchanged(
    root: &Path,
    inventory: &SourceInventory,
) -> Result<(), PresetStateError> {
    read_source(root, inventory, "studio.json")?
        .map(|_| ())
        .ok_or_else(|| style("studio.json", "missing from the project source"))
}

/// The reference the project's stored snapshot implies, if it has one.
pub fn stored_reference(root: &Path, inventory: &SourceInventory) -> Option<(String, String)> {
    let identity = current_identity(root, inventory).ok()??;
    Some((identity.id, identity.hash))
}

/// Builds the after file set for `request` against the current source. Pure apart from
/// reading the few style files it needs (each verified against the inventory).
pub fn plan(
    root: &Path,
    inventory: &SourceInventory,
    request: &PresetRequest<'_>,
) -> Result<PresetPlan, PresetStateError> {
    match request {
        PresetRequest::Select { package, action } => plan_select(root, inventory, package, *action),
        PresetRequest::SetOverride { name, value } => {
            plan_override(root, inventory, name, Some(value))
        }
        PresetRequest::ClearOverride { name } => plan_override(root, inventory, name, None),
    }
}

fn plan_select(
    root: &Path,
    inventory: &SourceInventory,
    package: &Package,
    action: PresetAction,
) -> Result<PresetPlan, PresetStateError> {
    let previous = current_identity(root, inventory)?;
    if action == PresetAction::Reapply {
        match &previous {
            None => return Err(PresetStateError::NoPreset),
            Some(applied) if applied.id != package.id() => {
                return Err(PresetStateError::WrongPreset {
                    applied: applied.id.clone(),
                    requested: package.id().to_owned(),
                });
            }
            Some(_) => (),
        }
    }
    let design: Design = package.manifest().design;
    let mut notes = Vec::new();
    // The user's file is kept as-is whenever it is not reset; it only feeds resolution
    // when it still reads against the new preset's design.
    let existing = read_source(root, inventory, OVERRIDES_PATH)?;
    let (overrides, keep): (OverridesFile, Option<Vec<u8>>) = match (action, existing) {
        (PresetAction::Reset, _) | (_, None) => (OverridesFile::default(), None),
        (_, Some(bytes)) => match OverridesFile::parse(&bytes, &design) {
            Ok(file) => (file, Some(bytes)),
            Err(error) => {
                notes.push(format!(
                    "style/overrides.json was kept untouched but does not read against {}: {error}",
                    package.id()
                ));
                (OverridesFile::default(), Some(bytes))
            }
        },
    };
    let materialized = studio_presets::materialize(package, &overrides)?;
    notes.extend(
        materialized
            .snapshot
            .diagnostics()
            .iter()
            .map(ToString::to_string),
    );
    let tokens_sha256 = materialized.snapshot.hash();
    let mut files = materialized.files;
    if let Some(bytes) = keep
        && let Some(entry) = files.iter_mut().find(|(p, _)| p.as_str() == OVERRIDES_PATH)
    {
        entry.1 = bytes;
    }
    let mut changes: Vec<FileChange> = Vec::new();
    // A preset's own resources are replaced as a set: drop what the previous preset
    // brought (its identity lists its flat media files; notices live under `style/preset/`)
    // and the new one does not. Any other file, including a user's own `media/` file, is
    // never deleted, and is never overwritten by a preset media file of the same name.
    let brought: Vec<&str> = previous
        .as_ref()
        .map(|p| p.media.iter().map(String::as_str).collect())
        .unwrap_or_default();
    for file in &inventory.files {
        let path = file.path.as_str();
        let owned =
            brought.contains(&path) || (previous.is_some() && path.starts_with(NOTICE_PREFIX));
        if owned && !files.iter().any(|(p, _)| p == &file.path) {
            changes.push(FileChange {
                path: file.path.clone(),
                bytes: None,
            });
        }
    }
    for (path, _) in &files {
        if path.as_str().starts_with(MEDIA_PREFIX)
            && !brought.contains(&path.as_str())
            && inventory.files.iter().any(|f| &f.path == path)
        {
            return Err(style(
                path.as_str(),
                "a project file with this name already exists and is not preset media; rename it before applying this preset",
            ));
        }
    }
    changes.extend(
        files
            .into_iter()
            .filter(|(path, bytes)| differs(inventory, path, bytes))
            .map(|(path, bytes)| FileChange {
                path,
                bytes: Some(bytes),
            }),
    );
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(PresetPlan {
        summary: format!("{} preset {}", action.label(), package.manifest().name),
        reference: Some((package.id().to_owned(), package.hash().to_owned())),
        provenance: PresetProvenance {
            action,
            preset_id: package.id().to_owned(),
            package_sha256: package.hash().to_owned(),
            tokens_sha256,
            previous_preset: previous.map(|p| p.id),
            notes,
        },
        changes,
    })
}

fn plan_override(
    root: &Path,
    inventory: &SourceInventory,
    name: &str,
    value: Option<&serde_json::Value>,
) -> Result<PresetPlan, PresetStateError> {
    let identity_bytes =
        read_source(root, inventory, PRESET_PATH)?.ok_or(PresetStateError::NoPreset)?;
    let defaults_bytes =
        read_source(root, inventory, PRESET_TOKENS_PATH)?.ok_or(PresetStateError::NoPreset)?;
    let identity: PresetIdentity = serde_json::from_slice(&identity_bytes)
        .map_err(|e| style(PRESET_PATH, format!("unreadable preset identity: {e}")))?;
    let defaults = studio_presets::TokenSet::parse(&defaults_bytes, &identity.design)?;
    let mut overrides = match read_source(root, inventory, OVERRIDES_PATH)? {
        Some(bytes) => OverridesFile::parse(&bytes, &identity.design).map_err(|e| {
            style(
                OVERRIDES_PATH,
                format!(
                    "existing overrides are unreadable ({e}); reset them or fix the file first"
                ),
            )
        })?,
        None => OverridesFile::default(),
    };
    let token = TokenName::new(name)?;
    let label = overrides.project().label().to_owned();
    let mut layer = studio_presets::OverrideLayer::new(label);
    for (existing, def) in overrides.project().defs() {
        if existing != &token {
            layer.insert(existing.clone(), def.clone());
        }
    }
    let action = match value {
        Some(value) => {
            let literal = parse_literal(token.kind(), &token, value, &identity.design)?;
            layer.insert(token.clone(), TokenDef::Literal(literal));
            PresetAction::SetOverride
        }
        None => {
            if overrides.project().defs().len() == layer.defs().len() {
                return Err(PresetStateError::NotAnOverride(name.to_owned()));
            }
            PresetAction::ClearOverride
        }
    };
    let snapshot = resolve(&defaults, Some(&layer), None);
    if action == PresetAction::SetOverride
        && snapshot.overridden_by().get(&token) != Some(&studio_presets::Layer::Project)
    {
        let why = snapshot
            .diagnostics()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(PresetStateError::NotApplied(if why.is_empty() {
            format!("`{name}` is not a token of this preset")
        } else {
            why
        }));
    }
    *overrides.project_mut() = layer;
    let mut identity = identity;
    identity.tokens_hash = snapshot.hash();
    let tokens_sha256 = identity.tokens_hash.clone();
    let candidates = [
        (TOKENS_PATH, snapshot.runtime_tokens_json().into_bytes()),
        (OVERRIDES_PATH, overrides.to_canonical_bytes()),
        (
            PRESET_PATH,
            serde_json::to_vec(&identity).map_err(|e| style(PRESET_PATH, e))?,
        ),
    ];
    let mut changes: Vec<FileChange> = candidates
        .into_iter()
        .map(|(path, bytes)| FileChange {
            path: project_path(path),
            bytes: Some(bytes),
        })
        .filter(|c| differs(inventory, &c.path, c.bytes.as_deref().unwrap_or_default()))
        .collect();
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(PresetPlan {
        summary: format!("{} {name}", action.label()),
        reference: None,
        provenance: PresetProvenance {
            action,
            preset_id: identity.id.clone(),
            package_sha256: identity.hash.clone(),
            tokens_sha256,
            previous_preset: Some(identity.id),
            notes: snapshot
                .diagnostics()
                .iter()
                .map(ToString::to_string)
                .collect(),
        },
        changes,
    })
}

// ---- the app-local store of imported presets ---------------------------------------------

/// Verifies the preset directory at `source` and publishes a copy in the app's preset
/// store (`<data>/presets/<id>-<package hash prefix>`). Importing a package that is
/// already stored returns it without writing. Nothing is stored unless the whole package
/// verified.
pub fn import_preset(
    paths: &crate::app_paths::AppPaths,
    source: &Path,
) -> Result<Package, PresetStateError> {
    let package = Package::from_files(studio_presets::package::read_dir_files(source)?)?;
    let dest = paths
        .presets()
        .join(format!("{}-{}", package.id(), &package.hash()[..12]));
    if dest.exists() {
        return Ok(package);
    }
    std::fs::create_dir_all(paths.presets()).map_err(|e| style("preset store", e))?;
    studio_presets::package::publish(&package, &dest)?;
    Ok(package)
}

/// Every verified package in the app's preset store, by name. A damaged entry is skipped
/// and reported in the second list instead of hiding the others.
pub fn installed_presets(paths: &crate::app_paths::AppPaths) -> (Vec<Package>, Vec<String>) {
    let mut packages = Vec::new();
    let mut skipped = Vec::new();
    let Ok(entries) = std::fs::read_dir(paths.presets()) else {
        return (packages, skipped);
    };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs.into_iter().filter(|p| p.is_dir()) {
        match studio_presets::package::read_dir_files(&dir).and_then(Package::from_files) {
            Ok(package) => packages.push(package),
            Err(error) => skipped.push(format!("{}: {error}", dir.display())),
        }
    }
    (packages, skipped)
}
