//! Rules the user saved with "Always in this project": requests of one kind (and, for a
//! tool, one title) that Studio answers itself in this project, with the agent's
//! `allow_once` option, whatever the permission policy.
//!
//! A rule is never broader than the request it was made from, and it never leaves the
//! working copy:
//! - a file request (`read`, `search`, `edit`, `delete`, `move`) gives a rule for that kind,
//!   which matches only requests whose every location (at least one) resolves inside the
//!   working copy;
//! - a tool the agent names (`other`, such as an MCP tool) gives a rule for that exact title,
//!   which matches only that title, with no location outside the working copy.
//!
//! Commands (`execute`), fetches, `think` and requests of no kind get no rule: their title
//! does not say what would run (Claude's command title is the agent's own description).
//!
//! The rules live in Studio's app data, beside the project's agent sessions
//! (`projects/<id>/agent/approvals.json`), never in the project's files. A file that does
//! not read is no rules; an entry that does not read, or that this module would never
//! write, is dropped.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use studio_harness::driver::PermissionPrompt;
use studio_project::ProjectId;

use crate::app_paths::AppPaths;

pub const APPROVALS_FILE: &str = "approvals.json";

/// Kinds whose rule covers the kind inside the working copy.
const FILE_KINDS: [&str; 5] = ["read", "search", "edit", "delete", "move"];
/// The kind whose rule covers one tool, named by the request's title.
const TOOL_KIND: &str = "other";

/// One saved "Always in this project".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRule {
    /// The request's kind: one of the file kinds, or `other` for a tool.
    pub kind: String,
    /// The tool's title, exactly as the agent sent it; only for `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// When it was saved (seconds since the Unix epoch); 0 when unknown.
    #[serde(default)]
    pub saved_unix: u64,
}

impl ApprovalRule {
    /// The rule "Always in this project" would save for `prompt`, or `None` when the
    /// request cannot be described safely: an unknown or command kind, a location outside
    /// `draft` (the working copy), or no `allow_once` option to answer with.
    pub(crate) fn for_request(prompt: &PermissionPrompt, draft: &Path) -> Option<Self> {
        if !prompt
            .options
            .iter()
            .any(|option| option.kind == "allow_once")
        {
            return None;
        }
        let kind = prompt.kind.as_deref()?;
        let rule = if FILE_KINDS.contains(&kind) {
            ApprovalRule {
                kind: kind.to_owned(),
                title: None,
                saved_unix: 0,
            }
        } else if kind == TOOL_KIND && !prompt.title.trim().is_empty() {
            ApprovalRule {
                kind: kind.to_owned(),
                title: Some(prompt.title.clone()),
                saved_unix: 0,
            }
        } else {
            return None;
        };
        rule.matches(prompt, draft).then_some(rule)
    }

    /// Whether this is a rule [`Self::for_request`] could have made.
    pub fn is_valid(&self) -> bool {
        match &self.title {
            None => FILE_KINDS.contains(&self.kind.as_str()),
            Some(title) => self.kind == TOOL_KIND && !title.trim().is_empty(),
        }
    }

    /// The same rule, whenever it was saved.
    pub fn same_as(&self, other: &ApprovalRule) -> bool {
        self.kind == other.kind && self.title == other.title
    }

    /// Whether the rule covers `prompt` in the working copy `draft`: the same kind (and
    /// title, for a tool), and every location inside the draft (a file rule also needs one).
    pub(crate) fn matches(&self, prompt: &PermissionPrompt, draft: &Path) -> bool {
        if !self.is_valid() || prompt.kind.as_deref() != Some(self.kind.as_str()) {
            return false;
        }
        if let Some(title) = &self.title
            && *title != prompt.title
        {
            return false;
        }
        let file_rule = self.title.is_none();
        if file_rule && prompt.locations.is_empty() {
            return false;
        }
        super::auto_answer::all_inside(draft, &prompt.locations)
    }

    /// What the rule allows, for the settings list.
    pub fn description(&self) -> String {
        match (self.kind.as_str(), &self.title) {
            ("read", None) => "Read files in the working copy".to_owned(),
            ("search", None) => "Search the working copy".to_owned(),
            ("edit", None) => "Edit files in the working copy".to_owned(),
            ("delete", None) => "Delete files in the working copy".to_owned(),
            ("move", None) => "Move files in the working copy".to_owned(),
            // Claude names MCP tools `mcp__<server>__<tool>`.
            (_, Some(title)) => match title
                .strip_prefix("mcp__")
                .and_then(|rest| rest.split_once("__"))
            {
                Some((server, tool)) => format!("Use “{tool}” from the {server} server"),
                None => format!("Use the tool “{title}”"),
            },
            (kind, None) => kind.to_owned(),
        }
    }
}

/// Where a project's rules are kept.
pub fn approvals_path(paths: &AppPaths, project: &ProjectId) -> PathBuf {
    paths.agent(project).join(APPROVALS_FILE)
}

#[derive(Default, Serialize, Deserialize)]
struct ApprovalsFile {
    #[serde(default)]
    rules: Vec<serde_json::Value>,
}

/// The project's saved rules, oldest first. A missing or unreadable file is no rules; an
/// entry that does not read or is not a valid rule is dropped, and a repeat is kept once.
pub fn load(paths: &AppPaths, project: &ProjectId) -> Vec<ApprovalRule> {
    let file: ApprovalsFile = std::fs::read_to_string(approvals_path(paths, project))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut rules: Vec<ApprovalRule> = Vec::new();
    for entry in file.rules {
        if let Ok(rule) = serde_json::from_value::<ApprovalRule>(entry)
            && rule.is_valid()
            && !rules.iter().any(|kept| kept.same_as(&rule))
        {
            rules.push(rule);
        }
    }
    rules
}

/// Replaces the project's saved rules through a temporary file renamed over the old one.
pub fn save(paths: &AppPaths, project: &ProjectId, rules: &[ApprovalRule]) -> Result<(), String> {
    let path = approvals_path(paths, project);
    let dir = path.parent().unwrap_or(&paths.data);
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot save the project's rules: {e}"))?;
    let text = serde_json::to_string_pretty(&serde_json::json!({ "rules": rules }))
        .map_err(|e| format!("Cannot encode the project's rules: {e}"))?;
    let mut file = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("Cannot save the project's rules: {e}"))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| format!("Cannot save the project's rules: {e}"))?;
    file.persist(&path)
        .map_err(|e| format!("Cannot save the project's rules: {}", e.error))?;
    Ok(())
}

/// Seconds since the Unix epoch, for [`ApprovalRule::saved_unix`].
pub(crate) fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_harness::driver::{PermissionChoice, PermissionId};

    pub(crate) fn prompt(
        kind: Option<&str>,
        title: &str,
        locations: &[&Path],
        options: &[&str],
    ) -> PermissionPrompt {
        PermissionPrompt {
            id: PermissionId(1),
            turn: 1,
            tool_call_id: "call".into(),
            title: title.into(),
            options: options
                .iter()
                .map(|kind| PermissionChoice {
                    option_id: format!("id-{kind}"),
                    name: (*kind).into(),
                    kind: (*kind).into(),
                })
                .collect(),
            kind: kind.map(str::to_owned),
            locations: locations.iter().map(|p| p.to_path_buf()).collect(),
        }
    }

    const OFFERED: [&str; 3] = ["allow_always", "allow_once", "reject_once"];

    fn draft() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("draft");
        std::fs::create_dir_all(draft.join("src")).unwrap();
        (dir, draft)
    }

    #[test]
    fn a_rule_is_offered_only_for_a_safe_request_with_a_single_allow() {
        let (dir, draft) = draft();
        let rule = |kind, title, locations: &[&Path], options: &[&str]| {
            ApprovalRule::for_request(&prompt(kind, title, locations, options), &draft)
        };
        let edit = rule(
            Some("edit"),
            "Edit a.rs",
            &[Path::new("src/a.rs")],
            &OFFERED,
        )
        .unwrap();
        assert_eq!((edit.kind.as_str(), edit.title.as_deref()), ("edit", None));
        let tool = rule(Some("other"), "mcp__github__list_issues", &[], &OFFERED).unwrap();
        assert_eq!(tool.title.as_deref(), Some("mcp__github__list_issues"));
        // A location outside the working copy, or none for a file kind.
        let outside = dir.path().join("x.rs");
        assert_eq!(rule(Some("edit"), "e", &[&outside], &OFFERED), None);
        assert_eq!(
            rule(Some("edit"), "e", &[Path::new("../x.rs")], &OFFERED),
            None
        );
        assert_eq!(rule(Some("edit"), "e", &[], &OFFERED), None);
        assert_eq!(
            rule(Some("other"), "mcp__x__w", &[&outside], &OFFERED),
            None
        );
        // Kinds whose title does not say what runs.
        for kind in ["execute", "fetch", "think"] {
            assert_eq!(rule(Some(kind), "Bash: run tests", &[], &OFFERED), None);
        }
        assert_eq!(rule(None, "t", &[Path::new("src/a.rs")], &OFFERED), None);
        assert_eq!(rule(Some("other"), "  ", &[], &OFFERED), None);
        // No `allow_once` to answer with.
        let always_only = ["allow_always", "reject_once"];
        assert_eq!(
            rule(Some("edit"), "e", &[Path::new("src/a.rs")], &always_only),
            None
        );
    }

    #[test]
    fn a_rule_never_matches_another_kind_or_tool() {
        let (_dir, draft) = draft();
        let edit = ApprovalRule {
            kind: "edit".into(),
            title: None,
            saved_unix: 1,
        };
        let inside = [Path::new("src/a.rs")];
        assert!(edit.matches(&prompt(Some("edit"), "any", &inside, &OFFERED), &draft));
        for kind in ["read", "delete", "move", "execute", "other"] {
            assert!(!edit.matches(&prompt(Some(kind), "any", &inside, &OFFERED), &draft));
        }
        assert!(!edit.matches(&prompt(None, "any", &inside, &OFFERED), &draft));
        let tool = ApprovalRule {
            kind: "other".into(),
            title: Some("mcp__github__list_issues".into()),
            saved_unix: 1,
        };
        let request = |kind, title| prompt(Some(kind), title, &[], &OFFERED);
        assert!(tool.matches(&request("other", "mcp__github__list_issues"), &draft));
        assert!(!tool.matches(&request("other", "mcp__github__delete_repo"), &draft));
        assert!(!tool.matches(&request("other", "mcp__github__list_issues "), &draft));
        assert!(!tool.matches(&request("execute", "mcp__github__list_issues"), &draft));
        assert_eq!(
            tool.description(),
            "Use “list_issues” from the github server"
        );
        // A rule nobody could have saved matches nothing.
        let broad = ApprovalRule {
            kind: "execute".into(),
            title: None,
            saved_unix: 1,
        };
        assert!(!broad.matches(&request("execute", "rm -rf /"), &draft));
    }

    #[test]
    fn rules_survive_a_save_per_project_and_a_corrupt_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        let one = ProjectId::try_from("project-one".to_owned()).unwrap();
        let two = ProjectId::try_from("project-two".to_owned()).unwrap();
        assert!(load(&paths, &one).is_empty());
        let rules = vec![
            ApprovalRule {
                kind: "edit".into(),
                title: None,
                saved_unix: 10,
            },
            ApprovalRule {
                kind: "other".into(),
                title: Some("mcp__x__y".into()),
                saved_unix: 20,
            },
        ];
        save(&paths, &one, &rules).unwrap();
        assert_eq!(load(&paths, &one), rules);
        assert!(load(&paths, &two).is_empty(), "rules belong to one project");
        let agent_dir = paths.agent(&one);
        let files: Vec<_> = std::fs::read_dir(&agent_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, [APPROVALS_FILE], "no temporary file is left");

        let path = approvals_path(&paths, &one);
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&paths, &one).is_empty());
        std::fs::write(&path, r#"{"rules": 3}"#).unwrap();
        assert!(load(&paths, &one).is_empty());
        // Good entries are kept; bad, broad and repeated ones are dropped.
        std::fs::write(
            &path,
            r#"{"rules": [
                {"kind": "edit", "saved_unix": 5},
                {"kind": "execute"},
                {"kind": "other"},
                {"kind": "edit", "title": "Edit a.rs"},
                {"title": 4},
                "edit",
                {"kind": "edit", "saved_unix": 6},
                {"kind": "read", "later": true}
            ], "version": 9}"#,
        )
        .unwrap();
        let loaded = load(&paths, &one);
        assert_eq!(
            loaded
                .iter()
                .map(|rule| (rule.kind.as_str(), rule.saved_unix))
                .collect::<Vec<_>>(),
            [("edit", 5), ("read", 0)]
        );
    }
}
