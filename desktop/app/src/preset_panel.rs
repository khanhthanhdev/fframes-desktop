//! Native style-preset controls: bundled and imported preset selection, the CSS import
//! report, export, basic project override editing and an explicit reset.
//!
//! Like the conversation panel this module never touches project files itself. Every
//! mutation goes through [`Controller::apply_preset`], the source-fenced durable engine
//! API (the fence is the source revision the UI last showed), and every command is run off
//! the UI thread by [`execute`]. Only a *committed* preset revision asks for a matching
//! preview ([`PresetReport::preview_request`]); a refused, conflicted or unchanged
//! application requests nothing, and a preview that then fails leaves the previous
//! playable preview on screen under the shell's "awaiting preview" label. The video
//! preset's tokens never reach the Studio chrome: the panel uses the application design
//! system, separate from project styling.
//!
//! * The **model** half (everything above [`PresetPanel`]) is GPUI-free and unit-testable.
//! * [`PresetPanel`] renders a [`PresetView`] and emits [`PresetEvent`]s for the shell.
use crate::design_system::colors::{
    BORDER, DANGER, DANGER_TEXT, MUTED, PANEL, SUCCESS, TEXT, WARNING,
};
use crate::{
    design_system::{self, ButtonStyle},
    text_input::TextInput,
};
use gpui::{
    AppContext, Context, ElementId, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use parking_lot::Mutex;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use studio_engine::{
    Controller, EngineError, PresetAction, PresetMutation, PresetRequest, PromotionError,
    app_paths::AppPaths, preset_state,
};
use studio_presets::{CssStatus, Design, Package, builtin, css_import::MAX_CSS_BYTES};
use studio_project::SourceRevision;

const MAX_CSS_LINES: usize = 40;

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
        EngineError::Promotion(PromotionError::Plan(studio_engine::PlanError::Conflict {
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

// ---- GPUI panel ------------------------------------------------------------------------

/// Native pickers the panel asks the shell to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetPick {
    ImportFolder,
    ImportCss,
    Export { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetEvent {
    Run(PresetCommand),
    Pick(PresetPick),
    /// Escape in an input: focus returns to the shell.
    Leave,
}

impl EventEmitter<PresetEvent> for PresetPanel {}

pub struct PresetPanel {
    view: Option<PresetView>,
    selected: Option<String>,
    last: Option<PresetReport>,
    css: Option<CssView>,
    token: Entity<TextInput>,
    value: Entity<TextInput>,
}

fn input(placeholder: &str, cx: &mut Context<PresetPanel>) -> Entity<TextInput> {
    let placeholder = placeholder.to_owned();
    cx.new(move |cx| {
        let mut input = TextInput::new(cx);
        input.placeholder = placeholder.into();
        input.compact = true;
        input
    })
}

impl PresetPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            view: None,
            selected: None,
            last: None,
            css: None,
            token: input("Token, for example color.accent", cx),
            value: input("Value, for example #ff5533 or 120", cx),
        }
    }

    // ---- shell interface ---------------------------------------------------------------

    /// The latest presentation of the open project (`None` when none is open).
    pub fn set_view(&mut self, view: Option<PresetView>, cx: &mut Context<Self>) {
        match &view {
            None => {
                self.last = None;
                self.css = None;
                self.selected = None;
            }
            Some(view) => {
                let still = self
                    .selected
                    .as_ref()
                    .is_some_and(|key| view.entries.iter().any(|e| &e.key == key));
                if !still {
                    // Start on what the project uses, else the first bundled preset.
                    self.selected = view
                        .entries
                        .iter()
                        .find(|e| e.applied)
                        .or_else(|| view.entries.first())
                        .map(|e| e.key.clone());
                }
            }
        }
        self.view = view;
        cx.notify();
    }

    /// The outcome of the command the shell just ran.
    pub fn report(&mut self, report: PresetReport, cx: &mut Context<Self>) {
        if let Some(css) = &report.css {
            self.css = Some(css.clone());
        }
        self.last = Some(report);
        cx.notify();
    }

    pub fn selected_key(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// True while an override field holds keyboard focus.
    pub fn has_text_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        use gpui::Focusable;
        self.token.focus_handle(cx).is_focused(window)
            || self.value.focus_handle(cx).is_focused(window)
    }

    // ---- building blocks ---------------------------------------------------------------

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        enabled: bool,
        selected: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let label: SharedString = label.into();
        let action = Rc::new(action);
        let on_click = action.clone();
        design_system::button(
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .tab_index(0)
                .tab_stop(enabled)
                .text_xs(),
            if selected {
                ButtonStyle::Selected
            } else {
                ButtonStyle::Secondary
            },
            enabled,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if enabled {
                on_click(this, cx);
            }
        }))
        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
            if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                action(this, cx);
                cx.stop_propagation();
            }
        }))
        .child(label)
    }

    fn line(text: impl Into<SharedString>, color: u32) -> gpui::Div {
        div().text_xs().text_color(rgb(color)).child(text.into())
    }

    fn run(&mut self, command: PresetCommand, cx: &mut Context<Self>) {
        cx.emit(PresetEvent::Run(command));
    }

    fn set_override(&mut self, cx: &mut Context<Self>) {
        let token = self.token.read(cx).content().trim().to_owned();
        let value = self.value.read(cx).content().trim().to_owned();
        if !token.is_empty() && !value.is_empty() {
            self.run(PresetCommand::SetOverride { token, value }, cx);
        }
    }

    fn field(
        &self,
        id: &'static str,
        input: Entity<TextInput>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let field = input.clone();
        div()
            .id(id)
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if field.read(cx).is_composing() {
                        cx.stop_propagation();
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "enter" => {
                            this.set_override(cx);
                            cx.stop_propagation();
                        }
                        "tab" => {
                            if event.keystroke.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            cx.stop_propagation();
                        }
                        "escape" => {
                            cx.emit(PresetEvent::Leave);
                            cx.stop_propagation();
                        }
                        _ => (),
                    }
                }),
            )
            .child(input)
    }
}

impl Render for PresetPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id("preset-panel")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .child(div().text_sm().child("Style presets"))
            .child(Self::line(
                "Presets style the video only; Studio's own interface never changes.",
                MUTED,
            ));
        let Some(view) = self.view.clone() else {
            return root.child(Self::line("Open a project to choose a preset.", MUTED));
        };
        let enabled = view.mutation.is_enabled();
        if let PresetMutation::Disabled(reason) = &view.mutation {
            root = root.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(WARNING))
                    .text_xs()
                    .child(format!("Preset changes are disabled: {reason}")),
            );
        }
        match &view.applied {
            Some(applied) => {
                root = root.child(Self::line(
                    format!(
                        "Applied: {} {} ({})",
                        applied.name,
                        applied.version,
                        &applied.hash[..12]
                    ),
                    TEXT,
                ));
            }
            None => {
                root = root.child(Self::line(
                    "No preset applied yet; this project keeps its own styles until you choose one.",
                    MUTED,
                ));
            }
        }
        let mut list = div().flex().flex_col().gap_1();
        for entry in &view.entries {
            let key = entry.key.clone();
            let label = format!(
                "{}{} {} · {}{}",
                if entry.applied { "✓ " } else { "" },
                entry.name,
                entry.version,
                if entry.bundled { "bundled" } else { "imported" },
                if entry.applied { " · applied" } else { "" }
            );
            list = list.child(self.button(
                format!("preset-{}", entry.key),
                label,
                true,
                self.selected.as_deref() == Some(entry.key.as_str()),
                cx,
                move |this, cx| {
                    this.selected = Some(key.clone());
                    cx.notify();
                },
            ));
        }
        for skipped in &view.skipped {
            list = list.child(Self::line(format!("Skipped: {skipped}"), DANGER_TEXT));
        }
        root = root.child(list);
        let selected = self
            .selected
            .as_ref()
            .and_then(|key| view.entries.iter().find(|e| &e.key == key))
            .cloned();
        if let Some(entry) = &selected {
            root = root.child(Self::line(
                format!("{} — {}", entry.id, entry.description),
                MUTED,
            ));
        }
        let key = selected.as_ref().map(|e| e.key.clone());
        let can_select = enabled && key.is_some();
        let reapply = enabled && selected.as_ref().is_some_and(|e| e.same_id_applied);
        let reset = enabled && view.applied.is_some() && key.is_some();
        let mut actions = div().flex().gap_2().flex_wrap();
        let apply_key = key.clone();
        actions = actions.child(self.button(
            "preset-apply",
            "Apply",
            can_select,
            false,
            cx,
            move |this, cx| {
                if let Some(key) = apply_key.clone() {
                    this.run(PresetCommand::Apply { key }, cx);
                }
            },
        ));
        let reapply_key = key.clone();
        actions = actions.child(self.button(
            "preset-reapply",
            "Reapply (keep overrides)",
            reapply,
            false,
            cx,
            move |this, cx| {
                if let Some(key) = reapply_key.clone() {
                    this.run(PresetCommand::Reapply { key }, cx);
                }
            },
        ));
        let reset_key = key.clone();
        actions = actions.child(self.button(
            "preset-reset",
            "Reset overrides",
            reset,
            false,
            cx,
            move |this, cx| {
                if let Some(key) = reset_key.clone() {
                    this.run(PresetCommand::Reset { key }, cx);
                }
            },
        ));
        let export_key = key.clone();
        actions = actions
            .child(self.button(
                "preset-import",
                "Import preset folder…",
                true,
                false,
                cx,
                |_, cx| {
                    cx.emit(PresetEvent::Pick(PresetPick::ImportFolder));
                },
            ))
            .child(self.button(
                "preset-export",
                "Export selected…",
                key.is_some(),
                false,
                cx,
                move |_, cx| {
                    if let Some(key) = export_key.clone() {
                        cx.emit(PresetEvent::Pick(PresetPick::Export { key }));
                    }
                },
            ))
            .child(self.button(
                "preset-css",
                "CSS import report…",
                true,
                false,
                cx,
                |_, cx| {
                    cx.emit(PresetEvent::Pick(PresetPick::ImportCss));
                },
            ));
        root = root.child(actions);

        // Project overrides: the user's layer, kept across Reapply and reopen.
        root = root.child(div().text_sm().mt_1().child("Project overrides"));
        if let Some(applied) = &view.applied {
            if applied.overridden.is_empty() {
                root = root.child(Self::line("No project overrides.", MUTED));
            }
            for (token, layer, value) in &applied.overridden {
                let clear = token.clone();
                root = root.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(Self::line(format!("{token} · {layer} · {value}"), TEXT))
                        .child(self.button(
                            format!("clear-{token}"),
                            "Clear",
                            enabled,
                            false,
                            cx,
                            move |this, cx| {
                                this.run(
                                    PresetCommand::ClearOverride {
                                        token: clear.clone(),
                                    },
                                    cx,
                                );
                            },
                        )),
                );
            }
            for diagnostic in &applied.diagnostics {
                root = root.child(
                    div()
                        .p_1()
                        .rounded_md()
                        .bg(rgb(WARNING))
                        .text_xs()
                        .child(format!("Kept, not applied: {diagnostic}")),
                );
            }
            let token = self.field("override-token", self.token.clone(), cx);
            let value = self.field("override-value", self.value.clone(), cx);
            root = root.child(token).child(value).child(self.button(
                "override-set",
                "Set override",
                enabled,
                false,
                cx,
                |this, cx| this.set_override(cx),
            ));
        }

        if let Some(last) = &self.last {
            let (background, lines) = match &last.outcome {
                Outcome::Committed { .. } | Outcome::Unchanged { .. } | Outcome::Done(_) => {
                    (SUCCESS, last.status_lines())
                }
                Outcome::Refused(_) => (DANGER, last.status_lines()),
            };
            let mut status = div()
                .id("preset-status")
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(rgb(background));
            for line in lines {
                status = status.child(Self::line(line, TEXT));
            }
            root = root.child(status);
        }
        if let Some(css) = &self.css {
            let mut report = div()
                .id("css-report")
                .max_h(px(200.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .child(Self::line(
                    format!(
                        "CSS report · {}: {} accepted · {} unsupported · {} rejected",
                        css.file, css.accepted, css.unsupported, css.rejected
                    ),
                    TEXT,
                ));
            for line in &css.lines {
                report = report.child(Self::line(line.clone(), MUTED));
            }
            if css.truncated {
                report = report.child(Self::line(
                    format!("Showing the first {MAX_CSS_LINES} declarations."),
                    MUTED,
                ));
            }
            root = root.child(report);
        }
        root
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
