//! Tests for draft retention, immutable capture, source fences and handoff preparation choices.

use std::fs;
use studio_bootstrap::{TerminationReport, WriterOwnership};
use studio_engine::{
    TaskError, TaskState, WriterGoneEvidence,
    app_paths::AppPaths,
    build_materialization::sdk_pin,
    controller::{Controller, DraftPreparationChoice},
};
use studio_sdk::CompatibilityManifest;

struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    paths: AppPaths,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    Fixture {
        _temp: temp,
        root,
        paths,
    }
}

fn qualified() -> WriterOwnership {
    WriterOwnership::ProcessGroupContained {
        qualification: "test-adapter".into(),
    }
}

fn clean_termination() -> TerminationReport {
    TerminationReport {
        forced: false,
        direct_child_exited: true,
        group_empty: true,
        remaining: vec![],
    }
}

#[test]
fn prepare_with_choice_continues_captured_draft_and_preserves_archived_copy() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();

    // 1. Start initial task
    let first = controller.begin_agent_task("initial task").unwrap();
    controller
        .agent_writer_started(&first.identity, qualified())
        .unwrap();

    // Outgoing writer makes edits in the draft
    fs::write(first.draft.join("draft_only.txt"), "outgoing draft edit\n").unwrap();

    // 2. Outgoing writer finishes and is verified gone
    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "switch intent stop")
        .unwrap();
    controller
        .acknowledge_agent_writer_gone(&WriterGoneEvidence::QualifiedAndVerified {
            ownership: qualified(),
            termination: clean_termination(),
        })
        .unwrap();

    // 3. Capture the outgoing draft immutably to checkpoints before handoff
    let captured_draft_rev = controller.checkpoints.capture(&first.draft).unwrap();
    let expected_source_revision = controller.current_source_revision();
    let expected_accepted_revision = controller.state().accepted().clone();

    // 4. Start second task continuing from the captured draft revision
    let second = controller
        .begin_agent_task_scoped_observed_with_choice(
            "continued task",
            None,
            DraftPreparationChoice::FromHandoff {
                content_revision: captured_draft_rev.clone(),
                expected_source_revision,
                expected_accepted_revision,
            },
            |_| {},
        )
        .unwrap();

    // Working copy contains the captured draft edits
    let content = fs::read_to_string(second.draft.join("draft_only.txt")).unwrap();
    assert_eq!(content, "outgoing draft edit\n");

    // The previous draft was archived
    assert!(second.archived_previous.is_some());
    let archived_dir = second.archived_previous.unwrap();
    assert!(archived_dir.join("draft_only.txt").exists());

    // Both variants remain retrievable:
    // a) Immutable captured draft revision
    let inspect_staging = controller
        .checkpoints
        .draft(&captured_draft_rev, "inspect-staging")
        .unwrap();
    assert_eq!(
        fs::read_to_string(inspect_staging.join("draft_only.txt")).unwrap(),
        "outgoing draft edit\n"
    );

    // b) Accepted source revision remains untouched
    let accepted_staging = controller
        .checkpoints
        .draft(second.source_base.revision(), "accepted-staging")
        .unwrap();
    assert!(!accepted_staging.join("draft_only.txt").exists());
}

#[test]
fn prepare_with_choice_restart_from_accepted_materializes_fresh_and_preserves_archive() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();

    let first = controller.begin_agent_task("first task").unwrap();
    controller
        .agent_writer_started(&first.identity, qualified())
        .unwrap();
    fs::write(first.draft.join("unwanted.txt"), "abandoned edits\n").unwrap();

    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "stop")
        .unwrap();
    controller
        .acknowledge_agent_writer_gone(&WriterGoneEvidence::QualifiedAndVerified {
            ownership: qualified(),
            termination: clean_termination(),
        })
        .unwrap();

    let captured_rev = controller.checkpoints.capture(&first.draft).unwrap();

    let accepted_revision = controller.state().accepted().clone();
    let expected_source_revision = controller.current_source_revision();
    let expected_accepted_revision = accepted_revision.clone();

    // Start task from the exact accepted source revision, not the captured working draft.
    let second = controller
        .begin_agent_task_scoped_observed_with_choice(
            "fresh task",
            None,
            DraftPreparationChoice::FromHandoff {
                content_revision: accepted_revision,
                expected_source_revision,
                expected_accepted_revision,
            },
            |_| {},
        )
        .unwrap();

    // Working copy does not have abandoned edits
    assert!(!second.draft.join("unwanted.txt").exists());

    // Archived copy preserves the abandoned draft
    let archived = second.archived_previous.unwrap();
    assert!(archived.join("unwanted.txt").exists());

    // Captured revision in checkpoints is also retrievable
    let staged = controller
        .checkpoints
        .draft(&captured_rev, "recovery-staged")
        .unwrap();
    assert!(staged.join("unwanted.txt").exists());
}

#[test]
fn handoff_revision_choice_is_refused_when_source_changes_after_choice() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let selected_content = controller.current_source_revision();
    let expected_source = controller.current_source_revision();
    let expected_accepted = controller.state().accepted().clone();

    fs::write(f.root.join("src/lib.rs"), "external source change\n").unwrap();
    let result = controller.begin_agent_task_scoped_observed_with_choice(
        "stale handoff choice",
        None,
        DraftPreparationChoice::FromHandoff {
            content_revision: selected_content,
            expected_source_revision: expected_source,
            expected_accepted_revision: expected_accepted,
        },
        |_| {},
    );

    assert!(matches!(
        result,
        Err(studio_engine::EngineError::State(
            studio_engine::StateError::StaleResult
        ))
    ));
    assert!(controller.agent_draft_store().snapshot().unwrap().is_none());
}

#[test]
fn prepare_refuses_when_lease_held_or_writer_active() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();

    let first = controller.begin_agent_task("running task").unwrap();
    controller
        .agent_writer_started(&first.identity, qualified())
        .unwrap();

    // Trying to start another task while first task holds the lease is refused
    let err = controller.begin_agent_task("second task").unwrap_err();
    assert!(matches!(
        err,
        studio_engine::EngineError::Task(TaskError::LeaseHeld(_))
    ));
}

#[test]
fn capture_outgoing_draft_fails_when_draft_directory_missing() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    // No task started, draft dir does not exist
    let err = controller.capture_outgoing_draft();
    assert!(
        err.is_err(),
        "capture must fail when draft directory does not exist"
    );
}
