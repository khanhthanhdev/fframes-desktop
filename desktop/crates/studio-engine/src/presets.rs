//! Style-preset commands: the catalog of bundled and imported presets, what the controls
//! show ([`PresetView`]), and [`execute`], which runs one [`PresetCommand`] against the
//! open project.
//!
//! Every mutation goes through [`Controller::apply_preset`], the source-fenced durable
//! engine API (the fence is the source revision the UI last showed). Only a *committed*
//! preset revision asks for a matching preview ([`PresetReport::preview_request`]); a
//! refused, conflicted or unchanged application requests nothing.
use crate::{
    Controller, EngineError, PresetAction, PresetMutation, PresetRequest, PromotionError,
    app_paths::AppPaths, preset_state,
};
use parking_lot::Mutex;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use studio_presets::{CssStatus, Design, Package, builtin, css_import::MAX_CSS_BYTES};
use studio_project::SourceRevision;

/// How many declarations of an imported style sheet the report shows.
pub const MAX_CSS_LINES: usize = 40;

// ---- catalog ---------------------------------------------------------------------------

/// The presets the user can choose: the bundled, read-only ones plus the verified packages
/// imported into the app's preset store.
#[derive(Clone, Default)]
pub struct Catalog {
    imported: Vec<Arc<Package>>,
    skipped: Vec<String>,
}

impl Catalog {
    /// Reads the app's preset store; a damaged entry is reported, never fatal.
    pub fn load(paths: &AppPaths) -> Self {
        let (packages, skipped) = preset_state::installed_presets(paths);
        let mut catalog = Self {
            imported: Vec::new(),
            skipped,
        };
        for package in packages {
            // A store entry identical to a bundled preset adds nothing to the list.
            if builtin::packages().is_ok_and(|b| b.iter().any(|p| p.hash() == package.hash())) {
                continue;
            }
            catalog.imported.push(Arc::new(package));
        }
        catalog
    }

    fn packages(&self) -> impl Iterator<Item = (&Package, bool)> {
        builtin::packages()
            .ok()
            .into_iter()
            .flatten()
            .map(|p| (p, true))
            .chain(self.imported.iter().map(|p| (p.as_ref(), false)))
    }

    /// A package by its hash (the catalog key).
    pub fn find(&self, key: &str) -> Option<&Package> {
        self.packages().map(|(p, _)| p).find(|p| p.hash() == key)
    }

    pub fn entries(&self, applied: Option<&AppliedView>) -> Vec<EntryView> {
        self.packages()
            .map(|(p, bundled)| EntryView {
                key: p.hash().to_owned(),
                id: p.id().to_owned(),
                name: p.manifest().name.clone(),
                version: p.manifest().version.clone(),
                description: p.manifest().description.clone(),
                bundled,
                applied: applied.is_some_and(|a| a.hash == p.hash()),
                same_id_applied: applied.is_some_and(|a| a.id == p.id()),
                swatch: swatch(p),
            })
            .collect()
    }

    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    fn adopt(&mut self, package: Package) {
        let known = self.packages().any(|(p, _)| p.hash() == package.hash());
        if !known {
            self.imported.push(Arc::new(package));
        }
    }
}

// ---- view ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryView {
    /// Package hash: the stable selection key.
    pub key: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub bundled: bool,
    /// This exact package is what the project applied.
    pub applied: bool,
    /// The project applied a preset with this id (possibly another version).
    pub same_id_applied: bool,
    /// The preset's `color.background`, `color.text` and `color.accent`, each when it
    /// defines one, for a picture of it.
    pub swatch: [Option<studio_presets::Color>; 3],
}

fn swatch(package: &Package) -> [Option<studio_presets::Color>; 3] {
    let snapshot = package.snapshot(None, None);
    ["color.background", "color.text", "color.accent"].map(|name| match snapshot.get(name) {
        Some(studio_presets::TokenValue::Color(color)) => Some(*color),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub hash: String,
    /// `(token, layer, value as JSON)` of every override that replaced a preset default.
    pub overridden: Vec<(String, String, String)>,
    /// Overrides kept in the file but not applied (orphaned, wrong type, ...).
    pub diagnostics: Vec<String>,
}

/// Everything the panel draws, captured under one short controller lock.
#[derive(Debug, Clone)]
pub struct PresetView {
    pub entries: Vec<EntryView>,
    pub skipped: Vec<String>,
    pub applied: Option<AppliedView>,
    pub mutation: PresetMutation,
}

impl PresetView {
    pub fn capture(controller: &mut Controller, catalog: &Catalog) -> Self {
        let applied = controller.project_style().map(|style| AppliedView {
            id: style.identity.id,
            name: style.identity.name,
            version: style.identity.version,
            hash: style.identity.hash,
            overridden: style.overridden,
            diagnostics: style.diagnostics,
        });
        Self {
            entries: catalog.entries(applied.as_ref()),
            skipped: catalog.skipped().to_vec(),
            mutation: controller.preset_mutation(),
            applied,
        }
    }
}

// ---- commands --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetCommand {
    /// Choose the catalog preset `key` (merging the project's overrides).
    Apply {
        key: String,
    },
    /// Refresh the applied preset from the catalog package `key` (same id).
    Reapply {
        key: String,
    },
    /// Apply `key` and clear every project override.
    Reset {
        key: String,
    },
    /// `value` is JSON when it parses (`"#ff0000"`, `120`, an object), else plain text.
    SetOverride {
        token: String,
        value: String,
    },
    ClearOverride {
        token: String,
    },
    ImportFolder(PathBuf),
    Export {
        key: String,
        dest: PathBuf,
    },
    ImportCss(PathBuf),
    /// Make a preset from a website (an `http(s)://` address or a domain) or a file (a
    /// page, style sheet, brand document, design tokens, SVG or image) and add it to the
    /// catalog.
    FromSource(String),
}

/// Why a command did not change the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The project's source is not the revision the UI showed; nothing was written.
    SourceChanged(String),
    /// Publication halted on a conflict: every observed variant was retained.
    Conflict {
        reason: String,
        variants: Vec<String>,
    },
    /// Mutation is disabled or suspended (platform, gate, unresolved conflict, task).
    Blocked(String),
    /// The request itself is not acceptable (unknown token, unreadable package, ...).
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A new durable preset revision; the source is now `source`.
    Committed {
        action: PresetAction,
        source: String,
        notes: Vec<String>,
    },
    /// The project already held exactly this snapshot.
    Unchanged {
        notes: Vec<String>,
    },
    Refused(Refusal),
    /// A non-mutating command succeeded (import, export, CSS report).
    Done(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssView {
    pub file: String,
    pub accepted: usize,
    pub unsupported: usize,
    pub rejected: usize,
    /// One line per declaration, bounded.
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetReport {
    pub outcome: Outcome,
    pub css: Option<CssView>,
    /// The catalog changed (an import); the shell refreshes the view.
    pub catalog_changed: bool,
}

/// What a committed preset revision asks of the preview pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRequest {
    /// The "awaiting preview" label shown until the matching preview installs.
    pub label: String,
}

impl PresetReport {
    fn outcome(outcome: Outcome) -> Self {
        Self {
            outcome,
            css: None,
            catalog_changed: false,
        }
    }

    pub fn committed(&self) -> bool {
        matches!(self.outcome, Outcome::Committed { .. })
    }

    pub fn refused(&self) -> bool {
        matches!(self.outcome, Outcome::Refused(_))
    }

    /// A matching M2 preview is requested only for a committed revision. The previous
    /// playable preview stays displayed (and labeled) until that preview is ready; if
    /// it fails the label stays and the preview keeps playing the prior revision.
    pub fn preview_request(&self) -> Option<PreviewRequest> {
        match &self.outcome {
            Outcome::Committed { action, source, .. } => Some(PreviewRequest {
                label: format!(
                    "{} committed as source {source}; its preview is not ready, the preview still shows the previous revision.",
                    action.label()
                ),
            }),
            _ => None,
        }
    }

    /// The one-line status under the controls: whether the source committed or was
    /// retained as a conflict, with every note.
    pub fn status_lines(&self) -> Vec<String> {
        match &self.outcome {
            Outcome::Committed {
                action,
                source,
                notes,
            } => {
                let mut lines = vec![format!(
                    "{} committed (source {source}); a matching preview is being prepared.",
                    action.label()
                )];
                lines.extend(notes.iter().map(|n| format!("Note: {n}")));
                lines
            }
            Outcome::Unchanged { notes } => {
                let mut lines = vec!["The project already has this exact snapshot.".to_owned()];
                lines.extend(notes.iter().map(|n| format!("Note: {n}")));
                lines
            }
            Outcome::Done(text) => vec![text.clone()],
            Outcome::Refused(Refusal::SourceChanged(text)) => {
                vec![format!("Not applied: {text}. Nothing was written.")]
            }
            Outcome::Refused(Refusal::Conflict { reason, variants }) => {
                let mut lines = vec![format!(
                    "Not committed: publication stopped on a conflict ({reason}). The source was not changed by Studio and every variant was kept:"
                )];
                lines.extend(variants.iter().map(|v| format!("  {v}")));
                lines
            }
            Outcome::Refused(Refusal::Blocked(text)) => vec![format!("Not applied: {text}")],
            Outcome::Refused(Refusal::Invalid(text)) => vec![format!("Not applied: {text}")],
        }
    }
}

fn classify(error: EngineError) -> Refusal {
    match error {
        EngineError::Promotion(PromotionError::SourceChanged { expected, found }) => {
            Refusal::SourceChanged(format!(
                "the project source changed ({expected} -> {found}) since it was shown; refresh and try again"
            ))
        }
        EngineError::Promotion(PromotionError::Conflict(report)) => Refusal::Conflict {
            reason: report.reason.clone(),
            variants: report.variants.iter().map(|v| v.path.clone()).collect(),
        },
        EngineError::Promotion(
            error @ (PromotionError::GateBlocked(_)
            | PromotionError::Unresolved(_)
            | PromotionError::Journal(_)
            | PromotionError::Crashed),
        ) => Refusal::Blocked(error.to_string()),
        EngineError::Promotion(PromotionError::Plan(crate::PlanError::Conflict {
            path,
            reason,
        })) => Refusal::Conflict {
            reason: format!("{path}: {reason}"),
            variants: Vec::new(),
        },
        other => Refusal::Invalid(other.to_string()),
    }
}

fn parse_value(text: &str) -> serde_json::Value {
    let trimmed = text.trim();
    serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::Value::String(trimmed.to_owned()))
}

fn css_view(file: &Path, text: &str, design: &Design) -> CssView {
    let report = studio_presets::import_css(text, design);
    let line = |e: &studio_presets::CssEntry| {
        let status = match e.status {
            CssStatus::Accepted => "accepted",
            CssStatus::Unsupported => "unsupported",
            CssStatus::Rejected => "rejected",
        };
        let mut out = format!("line {}: {status} `{}`", e.line, e.property);
        if let Some(token) = &e.token {
            out.push_str(&format!(" -> {token}"));
        }
        if let Some(value) = &e.normalized {
            out.push_str(&format!(" = {value}"));
        }
        if !e.reason.is_empty() {
            out.push_str(&format!(" ({})", e.reason));
        }
        out
    };
    CssView {
        file: file.file_name().map_or_else(
            || file.display().to_string(),
            |n| n.to_string_lossy().into(),
        ),
        accepted: report.count(CssStatus::Accepted),
        unsupported: report.count(CssStatus::Unsupported),
        rejected: report.count(CssStatus::Rejected),
        lines: report
            .entries
            .iter()
            .take(MAX_CSS_LINES)
            .map(line)
            .collect(),
        truncated: report.entries.len() > MAX_CSS_LINES,
    }
}

/// Runs one command. `expected` is the source revision the UI last showed: the fence of
/// every mutation. Runs on a background thread and holds the controller only for the
/// engine call.
pub fn execute(
    command: PresetCommand,
    controller: &Mutex<Controller>,
    catalog: &mut Catalog,
    paths: &AppPaths,
    expected: Option<&SourceRevision>,
) -> PresetReport {
    let refuse = |refusal: Refusal| PresetReport::outcome(Outcome::Refused(refusal));
    let mutate = |request: PresetRequest<'_>| -> PresetReport {
        let Some(expected) = expected else {
            return refuse(Refusal::Blocked("Open a project first".into()));
        };
        match controller.lock().apply_preset(expected, request) {
            Ok(outcome) => match outcome.record {
                Some(record) => PresetReport::outcome(Outcome::Committed {
                    action: record
                        .preset
                        .as_ref()
                        .map_or(PresetAction::Apply, |p| p.action),
                    source: outcome.source.as_str()[..12].to_owned(),
                    notes: outcome.notes,
                }),
                None => PresetReport::outcome(Outcome::Unchanged {
                    notes: outcome.notes,
                }),
            },
            Err(error) => refuse(classify(error)),
        }
    };
    match command {
        PresetCommand::Apply { ref key }
        | PresetCommand::Reapply { ref key }
        | PresetCommand::Reset { ref key } => {
            let Some(package) = catalog.find(key) else {
                return refuse(Refusal::Invalid(
                    "That preset is no longer in the catalog; refresh the list".into(),
                ));
            };
            let action = match &command {
                PresetCommand::Apply { .. } => PresetAction::Apply,
                PresetCommand::Reapply { .. } => PresetAction::Reapply,
                _ => PresetAction::Reset,
            };
            let package = package.clone();
            mutate(PresetRequest::Select {
                package: &package,
                action,
            })
        }
        PresetCommand::SetOverride { token, value } => {
            let value = parse_value(&value);
            mutate(PresetRequest::SetOverride {
                name: token.trim(),
                value: &value,
            })
        }
        PresetCommand::ClearOverride { token } => {
            mutate(PresetRequest::ClearOverride { name: token.trim() })
        }
        PresetCommand::ImportFolder(dir) => match preset_state::import_preset(paths, &dir) {
            Ok(package) => {
                let text = format!(
                    "Imported preset {} {} ({}).",
                    package.manifest().name,
                    package.manifest().version,
                    &package.hash()[..12]
                );
                catalog.adopt(package);
                PresetReport {
                    catalog_changed: true,
                    ..PresetReport::outcome(Outcome::Done(text))
                }
            }
            Err(error) => refuse(Refusal::Invalid(format!("Import failed: {error}"))),
        },
        PresetCommand::Export { key, dest } => match catalog.find(&key) {
            None => refuse(Refusal::Invalid(
                "That preset is no longer in the catalog".into(),
            )),
            Some(package) => match studio_presets::export_dir(package, &dest) {
                Ok(()) => PresetReport::outcome(Outcome::Done(format!(
                    "Exported {} to {} with its licenses.",
                    package.manifest().name,
                    dest.display()
                ))),
                Err(error) => refuse(Refusal::Invalid(format!("Export failed: {error}"))),
            },
        },
        PresetCommand::FromSource(source) => {
            let made = crate::preset_source::derive_from(&source).and_then(|derived| {
                let stored = preset_state::store_preset(paths, derived.package.clone())
                    .map_err(|e| e.to_string())?;
                Ok((derived, stored))
            });
            match made {
                Err(error) => refuse(Refusal::Invalid(format!("No preset made: {error}"))),
                Ok((derived, package)) => {
                    let fonts = if derived.fonts.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " It names {}, which are not bundled; the preset uses {}'s font.",
                            derived.fonts.join(", "),
                            derived.base
                        )
                    };
                    let text = format!(
                        "Made preset {}: background {}, text {}, accent {}, on {}.{fonts} Select it and Apply to use it.",
                        package.manifest().name,
                        derived.background.to_hex(),
                        derived.text.to_hex(),
                        derived.accent.to_hex(),
                        derived.base,
                    );
                    catalog.adopt(package);
                    PresetReport {
                        catalog_changed: true,
                        ..PresetReport::outcome(Outcome::Done(text))
                    }
                }
            }
        }
        PresetCommand::ImportCss(file) => {
            let design = controller
                .lock()
                .project_style()
                .map_or(Design::HD, |s| s.identity.design);
            let mut bytes = Vec::new();
            let read = fs::File::open(&file)
                .and_then(|f| f.take(MAX_CSS_BYTES as u64 + 1).read_to_end(&mut bytes));
            match read {
                Err(error) => refuse(Refusal::Invalid(format!("{}: {error}", file.display()))),
                Ok(_) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let view = css_view(&file, &text, &design);
                    PresetReport {
                        css: Some(view.clone()),
                        ..PresetReport::outcome(Outcome::Done(format!(
                            "CSS report for {}: {} accepted, {} unsupported, {} rejected. Nothing was written to the project.",
                            view.file, view.accepted, view.unsupported, view.rejected
                        )))
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_values_are_json_when_they_parse_and_text_otherwise() {
        assert_eq!(parse_value(" 120 "), serde_json::json!(120));
        assert_eq!(parse_value("\"#ff0000\""), serde_json::json!("#ff0000"));
        assert_eq!(parse_value("#ff0000"), serde_json::json!("#ff0000"));
        assert_eq!(parse_value("{\"a\":1}"), serde_json::json!({"a": 1}));
    }

    #[test]
    fn only_a_committed_revision_requests_a_preview() {
        let committed = PresetReport::outcome(Outcome::Committed {
            action: PresetAction::Apply,
            source: "abcdef012345".into(),
            notes: vec![],
        });
        let label = committed.preview_request().unwrap().label;
        assert!(label.contains("abcdef012345") && label.contains("previous revision"));
        for outcome in [
            Outcome::Unchanged { notes: vec![] },
            Outcome::Done("x".into()),
            Outcome::Refused(Refusal::Blocked("x".into())),
            Outcome::Refused(Refusal::SourceChanged("x".into())),
            Outcome::Refused(Refusal::Conflict {
                reason: "x".into(),
                variants: vec![],
            }),
        ] {
            assert!(PresetReport::outcome(outcome).preview_request().is_none());
        }
    }
}
