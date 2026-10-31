//! Which permission requests Studio answers itself under the user's [`PermissionPolicy`].
//!
//! Only what the policy clearly covers is answered, always with the agent's own
//! `allow_once` option (never `allow_always`, which would change the agent's stored rules).
//! Anything unclear (no kind, no locations, a path that leaves the working copy, no
//! `allow_once` option) is left to the user. Questions never come here.
//!
//! The project's saved rules ([`ApprovalRule`], "Always in this project") are consulted
//! too, under every policy, with the same answer and the same working-copy checks.

use std::path::{Component, Path, PathBuf};

use studio_harness::driver::PermissionPrompt;
use studio_proto::PermissionPolicy;

use super::approvals::ApprovalRule;

/// Kinds "Allow in the working copy" covers when every location is inside the draft.
const WORKING_COPY_KINDS: [&str; 5] = ["read", "search", "edit", "delete", "move"];

/// The option Studio answers `prompt` with under `policy` or one of the project's saved
/// `rules` ("Always in this project", which apply under every policy), or `None` to ask the
/// user. `draft` is the task's working copy, the agent's working directory (relative
/// locations are resolved against it).
pub(crate) fn auto_answer(
    policy: PermissionPolicy,
    rules: &[ApprovalRule],
    prompt: &PermissionPrompt,
    draft: &Path,
) -> Option<String> {
    let covered = policy_covers(policy, prompt, draft)
        || rules.iter().any(|rule| rule.matches(prompt, draft));
    if !covered {
        return None;
    }
    prompt
        .options
        .iter()
        .find(|option| option.kind == "allow_once")
        .map(|option| option.option_id.clone())
}

/// The saved rule that covers `prompt` when the policy alone does not: what the transcript
/// names for an answer [`auto_answer`] gave because of a rule.
pub(crate) fn answering_rule<'a>(
    policy: PermissionPolicy,
    rules: &'a [ApprovalRule],
    prompt: &PermissionPrompt,
    draft: &Path,
) -> Option<&'a ApprovalRule> {
    if policy_covers(policy, prompt, draft) {
        return None;
    }
    rules.iter().find(|rule| rule.matches(prompt, draft))
}

fn policy_covers(policy: PermissionPolicy, prompt: &PermissionPrompt, draft: &Path) -> bool {
    match policy {
        PermissionPolicy::AskEveryTime => false,
        PermissionPolicy::AllowEverything => true,
        PermissionPolicy::AllowInWorkingCopy => match prompt.kind.as_deref() {
            Some("think") => true,
            Some(kind) if WORKING_COPY_KINDS.contains(&kind) => {
                !prompt.locations.is_empty() && all_inside(draft, &prompt.locations)
            }
            _ => false,
        },
    }
}

/// Whether every one of `locations` resolves inside `draft` (true when there are none).
pub(super) fn all_inside(draft: &Path, locations: &[PathBuf]) -> bool {
    draft.canonicalize().is_ok_and(|root| {
        locations
            .iter()
            .all(|location| resolves_inside(&root, location))
    })
}

/// Whether `location` (absolute, or relative to `root`) is `root` or inside it once links
/// are followed. A path that does not exist yet is judged by its nearest existing ancestor,
/// and its missing part must be plain names (no `..`).
fn resolves_inside(root: &Path, location: &Path) -> bool {
    let path = root.join(location);
    let mut existing = path.as_path();
    let mut missing_plain = true;
    loop {
        match existing.canonicalize() {
            Ok(real) => return missing_plain && real.starts_with(root),
            Err(_) => {
                missing_plain &= matches!(
                    existing.components().next_back(),
                    Some(Component::Normal(_))
                );
                let Some(parent) = existing.parent() else {
                    return false;
                };
                existing = parent;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use studio_harness::driver::{PermissionChoice, PermissionId};

    fn prompt(kind: Option<&str>, locations: &[&Path], options: &[&str]) -> PermissionPrompt {
        PermissionPrompt {
            id: PermissionId(1),
            turn: 1,
            tool_call_id: "call".into(),
            title: "t".into(),
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

    #[test]
    fn working_copy_allows_edits_inside_the_draft_once_and_asks_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("draft");
        std::fs::create_dir_all(draft.join("src")).unwrap();
        std::fs::write(draft.join("src/lib.rs"), "").unwrap();
        let policy = PermissionPolicy::AllowInWorkingCopy;
        let answer = |kind, locations: &[&Path]| {
            auto_answer(
                policy,
                &[],
                &prompt(Some(kind), locations, &OFFERED),
                &draft,
            )
        };
        let once = Some("id-allow_once".to_owned());
        // Existing file, relative and absolute; a new file in a new folder.
        assert_eq!(answer("edit", &[Path::new("src/lib.rs")]), once);
        assert_eq!(answer("edit", &[&draft.join("src/lib.rs")]), once);
        assert_eq!(answer("edit", &[Path::new("src/new/scene.rs")]), once);
        assert_eq!(answer("read", &[Path::new(".")]), once);
        assert_eq!(answer("think", &[]), once);
        // Leaving the draft, lexically or by a missing `..`, or not saying where.
        assert_eq!(answer("edit", &[Path::new("../outside.rs")]), None);
        assert_eq!(answer("edit", &[Path::new("src/new/../../../x.rs")]), None);
        assert_eq!(answer("edit", &[&dir.path().join("other.rs")]), None);
        assert_eq!(
            answer("edit", &[Path::new("src/lib.rs"), &dir.path().join("o")]),
            None
        );
        assert_eq!(answer("edit", &[]), None);
        // Kinds the policy never covers.
        assert_eq!(answer("execute", &[Path::new("src/lib.rs")]), None);
        assert_eq!(answer("fetch", &[]), None);
        assert_eq!(answer("other", &[Path::new("src/lib.rs")]), None);
        let unknown = prompt(None, &[Path::new("src/lib.rs")], &OFFERED);
        assert_eq!(auto_answer(policy, &[], &unknown, &draft), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_draft_counts_as_outside() {
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("draft");
        std::fs::create_dir_all(&draft).unwrap();
        std::os::unix::fs::symlink(dir.path(), draft.join("escape")).unwrap();
        let request = prompt(Some("edit"), &[Path::new("escape/x.rs")], &OFFERED);
        assert_eq!(
            auto_answer(PermissionPolicy::AllowInWorkingCopy, &[], &request, &draft),
            None
        );
    }

    #[test]
    fn the_other_policies_and_missing_options() {
        let dir = tempfile::tempdir().unwrap();
        let draft = PathBuf::from(dir.path());
        let command = prompt(Some("execute"), &[], &OFFERED);
        assert_eq!(
            auto_answer(PermissionPolicy::AskEveryTime, &[], &command, &draft),
            None
        );
        assert_eq!(
            auto_answer(PermissionPolicy::AllowEverything, &[], &command, &draft),
            Some("id-allow_once".to_owned())
        );
        let unknown = prompt(None, &[], &OFFERED);
        assert!(auto_answer(PermissionPolicy::AllowEverything, &[], &unknown, &draft).is_some());
        let edit = prompt(Some("edit"), &[Path::new("a.rs")], &OFFERED);
        assert_eq!(
            auto_answer(PermissionPolicy::AskEveryTime, &[], &edit, &draft),
            None
        );
        // Never `allow_always`, and nothing when the agent offers no single allow.
        let always_only = prompt(
            Some("edit"),
            &[Path::new("a.rs")],
            &["allow_always", "reject_once"],
        );
        assert_eq!(
            auto_answer(PermissionPolicy::AllowEverything, &[], &always_only, &draft),
            None
        );
    }

    #[test]
    fn a_saved_rule_answers_once_under_every_policy_and_only_inside_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let draft = dir.path().join("draft");
        std::fs::create_dir_all(draft.join("src")).unwrap();
        let rules = [
            ApprovalRule {
                kind: "edit".into(),
                title: None,
                saved_unix: 1,
            },
            ApprovalRule {
                kind: "other".into(),
                title: Some("mcp__github__list_issues".into()),
                saved_unix: 1,
            },
        ];
        let ask = PermissionPolicy::AskEveryTime;
        let once = Some("id-allow_once".to_owned());
        let edit = |locations: &[&Path], options: &[&str]| {
            let mut request = prompt(Some("edit"), locations, options);
            request.title = "Edit".into();
            auto_answer(ask, &rules, &request, &draft)
        };
        assert_eq!(edit(&[Path::new("src/a.rs")], &OFFERED), once);
        // Never `allow_always`, even when it is offered first; nothing without a single allow.
        assert_eq!(
            edit(&[Path::new("src/a.rs")], &["allow_always", "allow_once"]),
            once
        );
        assert_eq!(
            edit(&[Path::new("src/a.rs")], &["allow_always", "reject_once"]),
            None
        );
        // An edit rule still needs every location inside the draft.
        let outside = dir.path().join("outside.rs");
        assert_eq!(edit(&[Path::new("src/a.rs"), &outside], &OFFERED), None);
        assert_eq!(edit(&[Path::new("../x.rs")], &OFFERED), None);
        assert_eq!(edit(&[], &OFFERED), None);
        // Another kind or another tool is asked.
        let tool = |kind, title: &str| {
            let mut request = prompt(Some(kind), &[], &OFFERED);
            request.title = title.into();
            auto_answer(ask, &rules, &request, &draft)
        };
        assert_eq!(tool("other", "mcp__github__list_issues"), once);
        assert_eq!(tool("other", "mcp__github__delete_repo"), None);
        assert_eq!(tool("execute", "mcp__github__list_issues"), None);
        assert_eq!(tool("delete", "Edit"), None);
        // Without rules the same requests are asked.
        let request = prompt(Some("edit"), &[Path::new("src/a.rs")], &OFFERED);
        assert_eq!(auto_answer(ask, &[], &request, &draft), None);
        // The transcript names the rule only when the policy alone would have asked.
        assert_eq!(
            answering_rule(ask, &rules, &request, &draft),
            Some(&rules[0])
        );
        let working_copy = PermissionPolicy::AllowInWorkingCopy;
        assert_eq!(answering_rule(working_copy, &rules, &request, &draft), None);
    }
}
