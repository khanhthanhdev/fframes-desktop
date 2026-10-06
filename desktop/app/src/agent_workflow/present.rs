//! Pure conversions from engine/driver values to bounded, redacted presentation values,
//! and the deterministic prompts the workflow sends.
use super::model::*;
use studio_agent_spike::{
    AgentFailure, FailureKind as ProviderFailureKind, redact_sensitive_string,
};
use studio_engine::{
    AgentTaskContext, EngineError, PromotionError, TaskError,
    candidate_validation::{CandidateError, ChangeSet, FailureContext, ValidationReport},
    edit_transaction::PlanError,
};

/// Short form of a revision hash.
pub fn short(revision: &str) -> String {
    revision.chars().take(12).collect()
}

/// Redacts token patterns and bounds `text` to `max` bytes on a character boundary.
pub fn bounded(text: &str, max: usize) -> String {
    let text = redact_sensitive_string(text);
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub fn message(text: &str) -> String {
    bounded(text, MAX_MESSAGE_BYTES)
}

/// One-line summary of a brief for rows, queue entries and the task header.
pub fn brief_summary(brief: &str) -> String {
    let line = brief.split_whitespace().collect::<Vec<_>>().join(" ");
    bounded(&line, 160)
}

fn snake(debug: &str) -> String {
    let mut out = String::new();
    for (i, c) in debug.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A provider failure (already redacted by the driver) as a structured error.
pub fn provider_error(failure: &AgentFailure, retained: bool) -> StructuredError {
    let (title, action) = match failure.kind {
        ProviderFailureKind::SpawnFailed | ProviderFailureKind::MissingRuntime => (
            "The agent could not be started",
            "Check the adapter executable and its runtime in the agent setup, then check readiness again.",
        ),
        ProviderFailureKind::AuthRequired | ProviderFailureKind::AuthRejected => (
            "The agent is not authenticated",
            "Sign in with the provider's own setup flow, then check readiness again.",
        ),
        ProviderFailureKind::UnsupportedVersion => (
            "The agent does not speak ACP v1",
            "Use an adapter that negotiates ACP protocol version 1.",
        ),
        ProviderFailureKind::DeadlineExceeded => (
            "The agent did not answer in time",
            "Send the brief again; the working copy was kept.",
        ),
        ProviderFailureKind::ProcessExited => (
            "The agent process ended unexpectedly",
            "Send the brief again; the working copy was kept.",
        ),
        _ => (
            "The agent failed",
            "Check readiness in the agent setup, then send the brief again; the working copy was kept.",
        ),
    };
    let mut detail = failure.message.clone();
    if let Some(extra) = &failure.detail {
        detail.push('\n');
        detail.push_str(extra);
    }
    StructuredError {
        code: format!("provider_{}", snake(&format!("{:?}", failure.kind))),
        title: title.to_owned(),
        detail: message(&detail),
        action: Some(action.to_owned()),
        phase: Some(format!("{:?}", failure.phase)),
        retained,
    }
}

pub fn simple_error(
    code: &str,
    title: &str,
    detail: &str,
    action: Option<&str>,
    retained: bool,
) -> StructuredError {
    StructuredError {
        code: code.to_owned(),
        title: title.to_owned(),
        detail: message(detail),
        action: action.map(str::to_owned),
        phase: None,
        retained,
    }
}

/// Classifies an engine error into a stable code and a next step.
pub fn engine_error(error: &EngineError, retained: bool) -> StructuredError {
    let (code, title, action): (&str, &str, &str) = match error {
        EngineError::Promotion(p) => match p {
            PromotionError::GateBlocked(_) => (
                "apply_blocked",
                "Apply is blocked",
                "The candidate is retained; export it, or fix the folder's filesystem and retry Apply.",
            ),
            PromotionError::Unresolved(_) => (
                "unresolved_mutation",
                "An earlier edit is unresolved",
                "Resolve the earlier conflict, then retry.",
            ),
            PromotionError::SourceChanged { .. }
            | PromotionError::HistoryChanged
            | PromotionError::SavedHistoryChanged
            | PromotionError::Plan(PlanError::Conflict { .. }) => (
                "source_conflict",
                "The project changed while the task ran",
                "The candidate and working copy were kept. Start a new task from the current source.",
            ),
            PromotionError::Conflict(_) => (
                "publication_conflict",
                "Publication stopped on a conflict",
                "Every observed variant was kept in the project folder; inspect them, then resolve the conflict.",
            ),
            PromotionError::RolledBack(_) => (
                "publication_rolled_back",
                "The edit could not be published and was rolled back",
                "The project is unchanged. Retry or export the retained candidate.",
            ),
            PromotionError::Journal(_) => (
                "history_suspended",
                "History storage failed",
                "Reopen the project to recover; no further edits are applied until then.",
            ),
            PromotionError::Unauthorized(_) => (
                "not_authorized",
                "The validation does not authorize this candidate",
                "Run the task again.",
            ),
            PromotionError::UndoUnavailable(_) => (
                "undo_unavailable",
                "Undo is unavailable",
                "There is no accepted agent edit to undo.",
            ),
            PromotionError::UndoConflict(_) => (
                "undo_conflict",
                "Undo conflicts with later changes",
                "Later edits touched the same files; nothing was changed.",
            ),
            PromotionError::StalePreparation(_) => (
                "undo_stale",
                "The Undo preparation is stale",
                "Request Undo again.",
            ),
            _ => (
                "promotion_failed",
                "The edit was not applied",
                "The project is unchanged.",
            ),
        },
        EngineError::Task(task) => match task {
            TaskError::QuiescenceBlocked(_) => (
                "quiescence_blocked",
                "The agent could not be proven finished",
                "The working copy was kept and is locked until you confirm that nothing is writing to it.",
            ),
            TaskError::DraftUnsafe(_) | TaskError::DraftBusy => (
                "draft_unsafe",
                "The working copy is locked",
                "Confirm that no agent is still writing to it (recovery panel), then retry.",
            ),
            TaskError::InvalidBrief(_) => {
                ("invalid_brief", "The brief is not valid", "Edit the brief.")
            }
            _ => (
                "task_failed",
                "The task failed",
                "The working copy was kept.",
            ),
        },
        EngineError::TaskScope(_) => (
            "stale_task_scope",
            "The selected scope is stale",
            "The project source or compiled timeline changed after this brief was submitted. The saved scene/range was not remapped; reselect it and send the brief again.",
        ),
        EngineError::Candidate(CandidateError::DraftChanged) => (
            "draft_changed",
            "The working copy changed while it was captured",
            "Something kept writing into it; the copy was kept. Retry.",
        ),
        EngineError::Candidate(_) => (
            "capture_failed",
            "The result could not be captured",
            "The working copy was kept. Retry.",
        ),
        EngineError::Project(_) | EngineError::State(_) => (
            "source_unavailable",
            "The project source could not be read",
            "Fix the project folder and try again; nothing was changed.",
        ),
        EngineError::NewerFormat { .. } => (
            "newer_format",
            "The history was written by a newer Studio",
            "Update Studio. Source and history were not changed.",
        ),
        _ => (
            "engine_error",
            "The operation failed",
            "Nothing was changed.",
        ),
    };
    StructuredError {
        code: code.to_owned(),
        title: title.to_owned(),
        detail: message(&error.to_string()),
        action: Some(action.to_owned()),
        phase: None,
        retained,
    }
}

pub fn change_card(changes: &ChangeSet) -> ChangeCard {
    ChangeCard {
        entries: changes.entries.iter().take(64).cloned().collect(),
        total: changes.total,
        truncated: changes.truncated || changes.entries.len() > 64,
    }
}

pub fn validation_card(report: &ValidationReport) -> ValidationCard {
    let coverage = report.coverage().map(|c| CoverageView {
        total_frames: c.total_frames,
        requested_scope: c.requested_scope.clone(),
        requested_interval: c.requested_interval,
        requested_frames: c.requested_frames.clone(),
        rendered_frame_indexes: c.rendered_frames.clone(),
        rendered_frames: c.rendered_frames.len(),
        inspected_frames: c.inspected_frames,
        boundary_frames: c.boundary_frames.len(),
        playhead: c.playhead,
        broadened: c.broadened.map(|b| format!("{b:?}")),
        complete: c.complete,
    });
    let audio = report.audio().map(|a| AudioView {
        silent: a.silent,
        peak: a.peak,
        sample_rate: a.sample_rate,
        sample_count: a.sample_count,
        placement_verified: a.placement_verified,
        checks_passed: a.checks_passed.clone(),
    });
    let failure = report.failure().map(|f| FailureView {
        stage: f.stage,
        kind: f.kind,
        summary: message(&f.summary),
    });
    let summary = if report.passed() {
        match &coverage {
            Some(c) => format!(
                "Validated: {} of {} frames rendered, {} inspected{}",
                c.rendered_frames,
                c.total_frames,
                c.inspected_frames,
                if c.complete {
                    ""
                } else {
                    " (coverage partial)"
                }
            ),
            None => "Validated".to_owned(),
        }
    } else if let Some(f) = &failure {
        f.summary.clone()
    } else {
        message(
            &report
                .acceptance_gap()
                .unwrap_or_else(|| "validation did not pass".to_owned()),
        )
    };
    ValidationCard {
        passed: report.passed(),
        summary,
        repair_count: report.repair_count(),
        coverage,
        errors: report
            .errors()
            .iter()
            .take(MAX_CARD_ITEMS)
            .cloned()
            .map(|mut d| {
                d.message = message(&d.message);
                d
            })
            .collect(),
        errors_total: report.errors_total(),
        diagnostics_total: report.diagnostics_total(),
        warnings: report
            .warnings()
            .iter()
            .take(MAX_CARD_ITEMS)
            .map(|w| message(w))
            .collect(),
        capability_gaps: report
            .capability_gaps()
            .iter()
            .take(MAX_CARD_ITEMS)
            .map(|w| message(w))
            .collect(),
        audio,
        failure,
        frames_rendered: report.frames().len(),
        build_key: report.build().map(|b| short(&b.key_digest)),
        candidate: short(report.candidate().revision().as_str()),
    }
}

/// Shown when the adapter does not negotiate ACP image prompts.
pub const IMAGE_LIMITATION: &str = "Text-only: this adapter did not negotiate ACP image prompts, so before frames remain app-owned artifact references.";

/// The before-evidence paragraph of a scoped first prompt: ids, hashes, sizes and frame
/// indexes of artifacts rendered from the frozen base. Image bytes are separate ACP
/// content blocks and never appear in this text or the persisted transcript.
pub fn before_evidence_prompt(view: &EvidenceView, image_limitation: Option<&str>) -> String {
    let mut text = format!(
        "\nBefore evidence (frozen source base {}):\n",
        view.revision
    );
    match view.state {
        EvidenceState::Ready => {
            for artifact in &view.artifacts {
                text.push_str(&format!(
                    "- {}: frames {:?}, {}x{} PNG, {} bytes, sha256 {} (app artifact {})\n",
                    artifact.label,
                    artifact.frames,
                    artifact.width,
                    artifact.height,
                    artifact.bytes,
                    &artifact.sha256[..12],
                    artifact.id
                ));
            }
            if let Some(limitation) = image_limitation {
                text.push_str(limitation);
                text.push('\n');
            }
        }
        _ => text.push_str(&format!(
            "- unavailable: {}\n",
            view.note.as_deref().unwrap_or("not rendered")
        )),
    }
    text.push_str(
        "You can render the same frames of your working copy with the render_frame/render_strip tools to compare.\n",
    );
    text
}

const MAX_STYLE_FILE_BYTES: u64 = 64 * 1024;
const MAX_STYLE_TOKENS: usize = 60;

/// The resolved design tokens of the task's working copy (`style/tokens.json`), bounded:
/// names, types and compact values, so the agent binds semantic tokens instead of copying
/// literals. Unreadable or oversized style data is reported, never guessed.
pub(crate) fn style_section(draft: &std::path::Path) -> String {
    let path = draft.join("style/tokens.json");
    let unavailable = |why: &str| format!("Resolved style tokens unavailable: {why}.\n");
    let Ok(meta) = std::fs::metadata(&path) else {
        return unavailable("the working copy has no style/tokens.json");
    };
    if meta.len() > MAX_STYLE_FILE_BYTES {
        return unavailable("style/tokens.json exceeds the context bound");
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return unavailable("style/tokens.json could not be read");
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return unavailable("style/tokens.json is not valid JSON");
    };
    let Some(tokens) = value.get("tokens").and_then(|t| t.as_object()) else {
        return unavailable("style/tokens.json has no tokens object");
    };
    let mut text = String::from(
        "Resolved style tokens (read through fframes::Styles by semantic name; prefer these over literals):\n",
    );
    for (name, token) in tokens.iter().take(MAX_STYLE_TOKENS) {
        let kind = token.get("type").and_then(|t| t.as_str()).unwrap_or("?");
        let rendered = token
            .get("value")
            .map(|v| v.to_string())
            .unwrap_or_default();
        text.push_str(&format!("- {name} ({kind}): {}\n", message(&rendered)));
    }
    if tokens.len() > MAX_STYLE_TOKENS {
        text.push_str(&format!(
            "- … {} more tokens in style/tokens.json\n",
            tokens.len() - MAX_STYLE_TOKENS
        ));
    }
    text
}

/// The first prompt of a task: the user's brief verbatim, then bounded, deterministic
/// context: the frozen scope, the working copy's design tokens when it has `style/tokens.json`
/// (read at the task's start, when the working copy equals its base), what exists and the rules.
pub fn first_prompt(context: &AgentTaskContext, route: &ToolRoute) -> String {
    let mut text = context.brief.clone();
    text.push_str("\n\n---\n");
    text.push_str(&format!(
        "You are editing an fframes video project. Your working directory ({}) is a private working copy of the project; change files only there. Keep studio.json's project id and SDK pin unchanged.\n",
        context.draft.display()
    ));
    text.push_str(
        "When you are done, end your turn. The app then validates the result (compile, timeline, rendered frames, audio) and applies it. If you need a clarification, ask one concise question in plain text and end your turn WITHOUT changing files; the answer arrives as your next prompt.\n",
    );
    text.push_str(&context.scope.prompt_context());
    if context.draft.join("style/tokens.json").exists() {
        text.push_str(&style_section(&context.draft));
    }
    let list = |label: &str, files: &[studio_project::revision::SourceFile], text: &mut String| {
        if files.is_empty() {
            return;
        }
        text.push_str(label);
        for file in files.iter().take(50) {
            text.push_str(&format!("- {}\n", file.path.as_str()));
        }
        if files.len() > 50 {
            text.push_str(&format!("- … and {} more\n", files.len() - 50));
        }
    };
    list(
        "Instruction files in the project:\n",
        &context.instructions,
        &mut text,
    );
    list("Existing media assets:\n", &context.assets, &mut text);
    text.push_str(&route.describe());
    text
}

/// How the agent can reach the project tools in one session. Both routes carry only the
/// capability FILE path; the secret never leaves that owner-only file.
#[derive(Debug, Clone, Default)]
pub struct ToolRoute {
    /// The `fframes-studio` MCP server is in the session's `mcpServers`.
    pub mcp: bool,
    pub cli: Option<CliRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliRoute {
    /// The `studio-tools` executable.
    pub command: std::path::PathBuf,
    pub capability_file: std::path::PathBuf,
}

impl ToolRoute {
    fn describe(&self) -> String {
        let tools = crate::agent_tools::TOOL_NAMES.join(", ");
        let mut text = String::new();
        if self.mcp {
            text.push_str(&format!(
                "Project tools ({tools}) are available through the `fframes-studio` MCP server.\n"
            ));
        }
        if let Some(cli) = &self.cli {
            text.push_str(&format!(
                "Project tools ({tools}) are {}available on the command line: `{} --capability {} <tool> [--json '<params object>']` (the reply JSON is printed on stdout).\n",
                if self.mcp { "also " } else { "" },
                cli.command.display(),
                cli.capability_file.display()
            ));
        }
        text
    }
}

/// Where the workflow keeps its conversation log.
pub fn log_path(
    paths: &studio_engine::app_paths::AppPaths,
    project: &studio_project::ProjectId,
) -> std::path::PathBuf {
    paths.project(project).join("conversation.jsonl")
}

/// Text of a repair turn: the engine's deterministic, bounded failure context, plus the
/// tool routes of the repair session (a new session knows nothing of the first one).
pub fn repair_prompt(context: &FailureContext, route: &ToolRoute) -> String {
    let mut text = context.repair_prompt();
    text.push_str(&route.describe());
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_agent_spike::driver::Phase;

    fn artifact(label: &str, frames: Vec<usize>) -> EvidenceArtifact {
        EvidenceArtifact {
            label: label.into(),
            frames,
            id: "artifact-7".into(),
            bytes: 4096,
            sha256: "0123456789abcdef".repeat(4),
            width: 480,
            height: 270,
        }
    }

    #[test]
    fn ready_before_evidence_names_frames_hashes_and_the_text_only_limit_without_pixels_or_paths() {
        let text = before_evidence_prompt(
            &EvidenceView {
                state: EvidenceState::Ready,
                revision: "d58449c3e9ec".into(),
                artifacts: vec![
                    artifact("selected", vec![17, 20, 23]),
                    artifact("after-boundary", vec![33]),
                ],
                note: None,
            },
            Some(IMAGE_LIMITATION),
        );
        assert!(text.contains("frozen source base d58449c3e9ec"), "{text}");
        assert!(text.contains("selected: frames [17, 20, 23]"), "{text}");
        assert!(text.contains("after-boundary: frames [33]"), "{text}");
        assert!(text.contains("sha256 0123456789ab"), "{text}");
        assert!(text.contains(IMAGE_LIMITATION), "{text}");
        assert!(
            !text.contains("base64") && !text.contains("data:image"),
            "{text}"
        );
        assert!(
            !text.contains("/tmp") && !text.contains("/root") && !text.contains(".png"),
            "no project or artifact path: {text}"
        );
    }

    #[test]
    fn the_style_section_lists_bounded_tokens_and_reports_missing_or_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(style_section(dir.path()).contains("unavailable"));
        std::fs::create_dir_all(dir.path().join("style")).unwrap();
        std::fs::write(dir.path().join("style/tokens.json"), "not json").unwrap();
        assert!(style_section(dir.path()).contains("not valid JSON"));
        let tokens: serde_json::Map<String, serde_json::Value> = (0..70)
            .map(|i| {
                (
                    format!("spacing.s{i:02}"),
                    serde_json::json!({"type": "dimension", "value": i as f64}),
                )
            })
            .collect();
        std::fs::write(
            dir.path().join("style/tokens.json"),
            serde_json::json!({"schema": 1, "tokens": tokens}).to_string(),
        )
        .unwrap();
        let text = style_section(dir.path());
        assert!(text.contains("- spacing.s00 (dimension): 0.0"), "{text}");
        assert!(text.contains("… 10 more tokens"), "{text}");
        assert!(!text.contains("spacing.s69"), "{text}");
    }

    #[test]
    fn unavailable_before_evidence_says_why_and_never_pretends_to_have_frames() {
        let text = before_evidence_prompt(
            &EvidenceView {
                state: EvidenceState::Unavailable,
                revision: "d58449c3e9ec".into(),
                artifacts: Vec::new(),
                note: Some("the baseline build failed".into()),
            },
            Some(IMAGE_LIMITATION),
        );
        assert!(
            text.contains("unavailable: the baseline build failed"),
            "{text}"
        );
        assert!(!text.contains("sha256"), "{text}");
        assert!(!text.contains(IMAGE_LIMITATION), "{text}");
    }

    #[test]
    fn messages_are_redacted_and_bounded_on_a_character_boundary() {
        let secret = message("failed with token=sk-live-123456 and more");
        assert!(!secret.contains("sk-live-123456"), "{secret}");
        let long = "é".repeat(MAX_MESSAGE_BYTES);
        let bounded = message(&long);
        assert!(bounded.len() <= MAX_MESSAGE_BYTES + '…'.len_utf8());
        assert!(bounded.ends_with('…'));
        assert!(brief_summary("one\n   two\tthree").starts_with("one two three"));
        assert!(brief_summary(&"x".repeat(1000)).len() < 200);
    }

    #[test]
    fn provider_failures_become_structured_errors_with_a_next_step() {
        let mut failure = AgentFailure::new(
            ProviderFailureKind::ProcessExited,
            Phase::Prompt,
            "the adapter exited with code 7 api_key=hunter2",
        );
        failure.detail = Some("stderr tail".into());
        let error = provider_error(&failure, true);
        assert_eq!(error.code, "provider_process_exited");
        assert_eq!(error.phase.as_deref(), Some("Prompt"));
        assert!(error.retained && error.action.is_some());
        assert!(!error.detail.contains("hunter2"));
        assert!(error.detail.contains("stderr tail"));
        let auth = provider_error(
            &AgentFailure::new(ProviderFailureKind::AuthRequired, Phase::Session, "auth"),
            false,
        );
        assert_eq!(auth.code, "provider_auth_required");
        assert!(auth.action.unwrap().contains("Sign in"));
    }

    #[test]
    fn engine_errors_are_classified_by_what_the_user_can_do() {
        let blocked: EngineError = PromotionError::GateBlocked("no renameat2".into()).into();
        assert_eq!(engine_error(&blocked, true).code, "apply_blocked");
        let moved: EngineError = PromotionError::SourceChanged {
            expected: "aaaaaaaaaaaa".into(),
            found: "bbbbbbbbbbbb".into(),
        }
        .into();
        assert_eq!(engine_error(&moved, true).code, "source_conflict");
        let draft: EngineError = TaskError::DraftUnsafe("locked".into()).into();
        assert_eq!(engine_error(&draft, true).code, "draft_unsafe");
        let changed: EngineError = CandidateError::DraftChanged.into();
        assert_eq!(engine_error(&changed, true).code, "draft_changed");
    }
}
