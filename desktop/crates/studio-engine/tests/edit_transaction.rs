//! Apply / Undo behaviour: durable task revisions, policy, fences and the interleavings
//! an uncooperative editor can produce around each publication step. Crash-boundary
//! coverage lives in `task_recovery.rs`.
#![cfg(target_os = "linux")]

#[path = "support/tx_fixture.rs"]
mod tx_fixture;

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::Arc,
};
use studio_engine::{
    Boundary, CompletionOutcome, Controller, EngineError, PromotionError, ReviewPolicy, TaskState,
    TransactionKind, UndoStatus,
    candidate_validation::{CapturedCandidate, ValidationReport},
};
use tx_fixture::*;

const TOUCHED: &[&str] = &[
    "notes.txt",
    "added.txt",
    "old.txt",
    "run.sh",
    "media",
    "media/new",
    "media/new/clip.txt",
];

fn promotion(error: &EngineError) -> &PromotionError {
    match error {
        EngineError::Promotion(p) => p,
        other => panic!("not a promotion error: {other}"),
    }
}

fn apply(
    f: &Fx,
    c: &mut Controller,
    brief: &str,
    edits: impl FnOnce(&std::path::Path),
) -> studio_engine::Promotion {
    let (_, captured, report) = validated(f, c, brief, edits);
    match c.complete_validated_task(&captured, &report).unwrap() {
        CompletionOutcome::Applied(p) => *p,
        other => panic!("expected an applied revision, got {other:?}"),
    }
}

#[test]
fn apply_publishes_a_multi_file_set_as_a_validated_task_revision() {
    let f = fixture();
    let before = snapshot(&f.root);
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let saved = c.state().accepted().clone();
    let (context, captured, report) = validated(&f, &mut c, "Make the intro red", multi_file_edit);
    let outcome = c.complete_validated_task(&captured, &report).unwrap();
    let CompletionOutcome::Applied(promotion) = outcome else {
        panic!("default policy applies automatically");
    };

    // Bytes, permissions and the executable bit.
    assert_eq!(
        fs::read_to_string(f.root.join("notes.txt")).unwrap(),
        "edited by the agent\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("media/new/clip.txt")).unwrap(),
        "created\n"
    );
    assert!(!f.root.join("old.txt").exists());
    assert_eq!(
        fs::metadata(f.root.join("run.sh")).unwrap().mode() & 0o7777,
        0o755
    );
    assert_eq!(
        fs::metadata(f.root.join("notes.txt")).unwrap().mode() & 0o7777,
        0o644
    );
    assert!(internal_files(&f.root).is_empty());
    let after = snapshot(&f.root);
    assert_eq!(outside_task(&before, &after, TOUCHED), Vec::<String>::new());
    // The private file kept its 0600 mode and the Git metadata is byte-identical.
    assert_eq!(after["private.txt"].mode, 0o600);
    assert_eq!(
        differences(
            &before
                .iter()
                .filter(|(p, _)| p.starts_with(".git"))
                .map(|(p, n)| (p.clone(), n.clone()))
                .collect(),
            &after
                .iter()
                .filter(|(p, _)| p.starts_with(".git"))
                .map(|(p, n)| (p.clone(), n.clone()))
                .collect(),
        ),
        Vec::<String>::new()
    );

    // Source and saved checkpoint are semantically distinct.
    assert_eq!(c.state().source(), captured.candidate().revision());
    assert_eq!(c.state().accepted(), &saved);
    assert_ne!(c.state().source(), c.state().accepted());

    // The task revision names everything.
    let record = &promotion.record;
    assert_eq!(
        c.state().task_history().entries(),
        std::slice::from_ref(record)
    );
    assert_eq!(record.kind, TransactionKind::Apply);
    assert_eq!(record.task, context.identity.task.0);
    assert_eq!(&record.task_base, context.source_base.revision());
    assert_eq!(record.prior_history, None);
    assert_eq!(record.prior_checkpoint, saved);
    assert_eq!(&record.candidate, captured.candidate().revision());
    assert_eq!(record.published, record.candidate);
    assert_eq!(record.prompt_summary, "Make the intro red");
    assert_eq!(record.build, report.build().cloned());
    assert_eq!(record.validation_report_sha256.len(), 64);
    assert!(
        f.history(&c)
            .join("reports")
            .join(format!("{}.json", record.validation_report_sha256))
            .is_file()
    );
    let mut changed: Vec<_> = record.changed_paths().collect();
    changed.sort_unstable();
    assert_eq!(
        changed,
        [
            "added.txt",
            "media/new/clip.txt",
            "notes.txt",
            "old.txt",
            "run.sh"
        ]
    );
    let delta = |p: &str| record.changes.iter().find(|d| d.path == p).unwrap();
    assert!(delta("added.txt").before.is_none() && delta("added.txt").after.is_some());
    assert!(delta("old.txt").after.is_none() && delta("old.txt").before.is_some());
    let mode = delta("run.sh");
    assert_eq!(
        mode.before.as_ref().unwrap().sha256,
        mode.after.as_ref().unwrap().sha256
    );
    assert!(!mode.before.as_ref().unwrap().executable && mode.after.as_ref().unwrap().executable);

    // The task finished accepted, the draft is marked accepted, and the install
    // authorization names the published candidate, not the task base.
    assert_eq!(c.agent_task().unwrap().state(), TaskState::Accepted);
    let authorization = promotion.authorization.expect("fresh authorization");
    assert_eq!(authorization.published(), &record.published);
    assert_eq!(&authorization.tag().base_source, &record.published);
    assert_ne!(
        &authorization.tag().base_source,
        context.source_base.revision()
    );
    assert!(authorization.is_current(c.state()));
    assert!(promotion.notes.is_empty(), "{:?}", promotion.notes);

    // Durable across reopen, with a consistent projection.
    drop(c);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert_eq!(c.state().source(), captured.candidate().revision());
    assert_eq!(c.state().accepted(), &saved);
    assert!(!c.recovery_status().projection_rebuilt);
    assert!(c.recovery_status().unresolved.is_empty());
}

#[test]
fn review_policy_defaults_to_automatic_and_manual_review_waits_for_an_explicit_apply() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    assert_eq!(c.review_policy(), ReviewPolicy::AutoApply);
    assert!(ReviewPolicy::SCOPE_NOTICE.contains("does not travel"));
    c.set_review_policy(ReviewPolicy::ManualReview).unwrap();
    let before = snapshot(&f.root);
    let (_, captured, report) = validated(&f, &mut c, "manual", multi_file_edit);
    let outcome = c.complete_validated_task(&captured, &report).unwrap();
    assert!(matches!(outcome, CompletionOutcome::AwaitingReview));
    assert_eq!(
        snapshot(&f.root),
        before,
        "nothing is published before review"
    );
    assert_eq!(c.agent_task().unwrap().state(), TaskState::CandidateReady);
    assert!(c.state().task_history().entries().is_empty());
    // The policy is app-local and survives a reopen.
    let promotion = c.apply_candidate(&captured, &report).unwrap();
    assert_eq!(promotion.record.kind, TransactionKind::Apply);
    drop(c);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.review_policy(), ReviewPolicy::ManualReview);
}

#[test]
fn dirty_imported_bytes_are_the_task_base_and_the_old_checkpoint_is_never_restored() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let saved = c.state().accepted().clone();
    // Uncheckpointed edits and a new imported file exist when the task launches.
    fs::write(
        f.root.join("notes.txt"),
        "dirty edit made after the checkpoint\n",
    )
    .unwrap();
    fs::write(f.root.join("imported.txt"), "imported\n").unwrap();
    let dirty = fs::read(f.root.join("notes.txt")).unwrap();
    let promotion = apply(&f, &mut c, "keep the import", |draft| {
        assert_eq!(fs::read(draft.join("notes.txt")).unwrap(), dirty);
        fs::write(draft.join("added.txt"), "added\n").unwrap();
    });
    assert_eq!(fs::read(f.root.join("notes.txt")).unwrap(), dirty);
    assert_eq!(
        fs::read_to_string(f.root.join("imported.txt")).unwrap(),
        "imported\n"
    );
    assert!(f.root.join("added.txt").is_file());
    assert_eq!(
        c.state().accepted(),
        &saved,
        "applying never rewinds or advances the saved checkpoint"
    );
    assert_ne!(&promotion.record.task_base, &saved);
    assert_eq!(promotion.record.prior_checkpoint, saved);
    let changed: Vec<_> = promotion.record.changed_paths().collect();
    assert_eq!(changed, ["added.txt"]);
}

#[test]
fn a_source_edit_after_launch_refuses_with_the_candidate_preserved() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (context, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    fs::write(
        f.root.join("notes.txt"),
        "the user typed while the agent worked\n",
    )
    .unwrap();
    let before = snapshot(&f.root);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::SourceChanged { .. }),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before, "no source byte changed");
    assert_eq!(c.agent_task().unwrap().state(), TaskState::Conflict);
    assert!(c.state().task_history().entries().is_empty());
    // The candidate and the draft are preserved for review/export.
    assert!(
        f.checkpoints(&c)
            .load(captured.candidate().revision())
            .is_ok()
    );
    assert!(context.draft.join("notes.txt").is_file());
    assert!(internal_files(&f.root).is_empty());
}

#[test]
fn a_task_history_change_after_launch_fences_the_task() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    apply(&f, &mut c, "first", |d| {
        fs::write(d.join("added.txt"), "a\n").unwrap()
    });
    let (_, captured, report) = validated(&f, &mut c, "second", |d| {
        fs::write(d.join("second.txt"), "b\n").unwrap();
    });
    // While the second task is open the first edit is undone.
    let undo = c.prepare_undo(None).unwrap();
    let undo_report = passing_probe::passing_report(undo.captured());
    c.undo_task(&undo, &undo_report).unwrap();
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(matches!(
        promotion(&error),
        PromotionError::SourceChanged { .. } | PromotionError::HistoryChanged
    ));
    assert_eq!(c.agent_task().unwrap().state(), TaskState::Conflict);
    assert!(!f.root.join("second.txt").exists());
}

#[test]
fn a_nonpassing_or_foreign_report_never_authorizes_publication() {
    use studio_engine::candidate_validation::{
        BuildOutcome, FailureKind, ValidationStage, validate_candidate,
    };
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = ready(&f, &mut c, "edit", multi_file_edit);
    let before = snapshot(&f.root);
    // A failed report for the very same candidate.
    let failed = validate_candidate(
        &captured,
        BuildOutcome::Failed {
            kind: FailureKind::Source,
            stage: ValidationStage::Compile,
            output: "error[E0000]".into(),
            build: None,
        },
        0,
        0,
        None,
    );
    assert!(!failed.passed());
    let error = c.apply_candidate(&captured, &failed).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Unauthorized(_)),
        "{error}"
    );
    // A passing report for another project's task.
    let other = fixture();
    let mut other_controller = Controller::open(&other.root, &other.paths).unwrap();
    let (_, other_captured, other_report) =
        validated(&other, &mut other_controller, "other", multi_file_edit);
    let error = c.apply_candidate(&captured, &other_report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Unauthorized(_)),
        "{error}"
    );
    // A captured candidate that is not this task's cannot be applied either.
    let error = c
        .apply_candidate(&other_captured, &other_report)
        .unwrap_err();
    assert!(matches!(error, EngineError::Task(_)), "{error}");
    assert_eq!(snapshot(&f.root), before);
    assert_eq!(c.agent_task().unwrap().state(), TaskState::CandidateReady);
    // The genuine report still works afterwards.
    c.apply_candidate(&captured, &report).unwrap();
    assert_ne!(snapshot(&f.root), before);
}

#[test]
fn undo_reverses_only_its_own_files_and_keeps_unrelated_external_edits() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let first = apply(&f, &mut c, "Add the intro", |d| {
        fs::write(d.join("notes.txt"), "agent notes\n").unwrap();
        fs::write(d.join("added.txt"), "added\n").unwrap();
        fs::remove_file(d.join("old.txt")).unwrap();
        fs::set_permissions(d.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    });
    // The user edits an unrelated file and creates a new one afterwards.
    fs::write(f.root.join("private.txt"), "my later edit\n").unwrap();
    fs::write(f.root.join("scratch.txt"), "scratch\n").unwrap();
    let before_undo = snapshot(&f.root);
    let UndoStatus::Available { target, summary } = c.undo_status() else {
        panic!("undo should be available");
    };
    assert_eq!(target, first.record.id);
    assert_eq!(summary, "Add the intro");

    let undo = c.prepare_undo(None).unwrap();
    assert_eq!(undo.target(), first.record.id);
    // Nothing is published by preparing, and the candidate is a merged inventory.
    assert_eq!(snapshot(&f.root), before_undo);
    let report = passing_probe::passing_report(undo.captured());
    let promotion = c.undo_task(&undo, &report).unwrap();

    assert_eq!(
        fs::read_to_string(f.root.join("notes.txt")).unwrap(),
        "keep me\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("old.txt")).unwrap(),
        "to be deleted\n"
    );
    assert!(!f.root.join("added.txt").exists());
    assert_eq!(
        fs::metadata(f.root.join("run.sh")).unwrap().mode() & 0o7777,
        0o644
    );
    // Unrelated external changes survive, with their permissions.
    assert_eq!(
        fs::read_to_string(f.root.join("private.txt")).unwrap(),
        "my later edit\n"
    );
    assert_eq!(
        fs::metadata(f.root.join("private.txt")).unwrap().mode() & 0o7777,
        0o600
    );
    assert_eq!(
        fs::read_to_string(f.root.join("scratch.txt")).unwrap(),
        "scratch\n"
    );
    let after = snapshot(&f.root);
    assert_eq!(
        outside_task(
            &before_undo,
            &after,
            &["notes.txt", "old.txt", "added.txt", "run.sh"]
        ),
        Vec::<String>::new()
    );
    assert!(internal_files(&f.root).is_empty());

    // A new transition that points back at the undone revision; the resulting
    // inventory is the merged one, not the historical predecessor.
    let record = &promotion.record;
    assert_eq!(record.kind, TransactionKind::Undo);
    assert_eq!(record.undoes.as_deref(), Some(first.record.id.as_str()));
    assert_ne!(record.id, first.record.id);
    assert_eq!(
        record.prior_history.as_deref(),
        Some(first.record.id.as_str())
    );
    assert_ne!(record.published, first.record.task_base);
    assert_eq!(c.state().source(), &record.published);
    assert_eq!(c.state().task_history().entries().len(), 2);
    assert_eq!(record.prompt_summary, "Undo: Add the intro");
    assert!(promotion.authorization.is_some());
    // No-op Undo now explains itself.
    let UndoStatus::Unavailable { reason } = c.undo_status() else {
        panic!("everything is undone");
    };
    assert!(reason.contains("already been undone"), "{reason}");
    let error = c.prepare_undo(None).unwrap_err();
    assert!(matches!(
        promotion_of(&error),
        PromotionError::UndoUnavailable(_)
    ));
    // Undoing the Undo (redo) is an ordinary delta.
    let redo = c.prepare_undo(Some(&record.id)).unwrap();
    let report = passing_probe::passing_report(redo.captured());
    c.undo_task(&redo, &report).unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("notes.txt")).unwrap(),
        "agent notes\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("private.txt")).unwrap(),
        "my later edit\n"
    );
}

fn promotion_of(error: &EngineError) -> &PromotionError {
    promotion(error)
}

#[test]
fn undo_conflicts_on_a_touched_file_and_changes_nothing() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    apply(&f, &mut c, "edit", |d| {
        fs::write(d.join("notes.txt"), "agent notes\n").unwrap();
        fs::write(d.join("added.txt"), "added\n").unwrap();
    });
    fs::write(
        f.root.join("notes.txt"),
        "the user rewrote the agent's file\n",
    )
    .unwrap();
    let before = snapshot(&f.root);
    let error = c.prepare_undo(None).unwrap_err();
    let PromotionError::UndoConflict(conflicts) = promotion(&error) else {
        panic!("{error}");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "notes.txt");
    assert!(error.to_string().contains("notes.txt"));
    assert_eq!(
        snapshot(&f.root),
        before,
        "conflicting touched files stay untouched"
    );
    assert_eq!(c.state().task_history().entries().len(), 1);
}

#[test]
fn an_external_edit_between_preparing_and_publishing_undo_refuses() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    apply(&f, &mut c, "edit", |d| {
        fs::write(d.join("added.txt"), "added\n").unwrap()
    });
    let undo = c.prepare_undo(None).unwrap();
    let report = passing_probe::passing_report(undo.captured());
    fs::write(f.root.join("scratch.txt"), "typed after validation\n").unwrap();
    let before = snapshot(&f.root);
    let error = c.undo_task(&undo, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::SourceChanged { .. }),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
}

// ---- interleavings with an uncooperative writer ---------------------------------------

/// Runs the standard multi-file Apply with `hooks` installed and returns the controller,
/// the pre-Apply snapshot and the error (Apply must not succeed).
fn raced(
    f: &Fx,
    hooks: Tape,
) -> (
    Controller,
    std::collections::BTreeMap<String, Node>,
    EngineError,
    CapturedCandidate,
    ValidationReport,
) {
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(f, &mut c, "edit", multi_file_edit);
    let before = snapshot(&f.root);
    c.set_transaction_hooks(hooks.hooks());
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    (c, before, error, captured, report)
}

/// `history` is how many task revisions were accepted before the raced Apply started.
fn assert_conflict_preserved(
    c: &Controller,
    f: &Fx,
    error: &EngineError,
    history: usize,
    variants: &[&[u8]],
) {
    assert!(
        matches!(promotion(error), PromotionError::Conflict(_)),
        "{error}"
    );
    assert_eq!(c.agent_task().unwrap().state(), TaskState::Conflict);
    assert_eq!(
        c.state().task_history().entries().len(),
        history,
        "acceptance never advanced"
    );
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    for bytes in variants {
        assert!(
            survives(&f.root, bytes),
            "variant {:?} was lost",
            String::from_utf8_lossy(bytes)
        );
    }
    assert!(c.ensure_blocked());
}

trait Blocked {
    fn ensure_blocked(&self) -> bool;
}
impl Blocked for Controller {
    fn ensure_blocked(&self) -> bool {
        !self.recovery_status().unresolved.is_empty()
    }
}

#[test]
fn an_in_place_write_between_verification_and_displacement_is_retained() {
    let f = fixture();
    let root = f.root.clone();
    let tape = Tape::on(
        |b| matches!(b, Boundary::BeforeDisplace(_)),
        move || {
            // The editor saves in place right before we displace the file.
            let target = ["notes.txt", "old.txt", "run.sh"]
                .into_iter()
                .find(|n| fs::read(root.join(n)).is_ok_and(|b| !b.starts_with(b"EDITOR")))
                .unwrap();
            let mut text = b"EDITOR wrote this in place\n".to_vec();
            text.extend_from_slice(target.as_bytes());
            fs::write(root.join(target), text).unwrap();
        },
    );
    let (c, _, error, _, _) = raced(&f, tape);
    assert_conflict_preserved(&c, &f, &error, 0, &[]);
    let found = all_contents(&f.root)
        .into_iter()
        .any(|b| b.starts_with(b"EDITOR wrote this in place"));
    assert!(found, "the editor's bytes were lost");
}

#[test]
fn an_editor_save_by_rename_during_displacement_keeps_the_new_file_in_place() {
    let f = fixture();
    let root = f.root.clone();
    let tape = Tape::on(
        |b| matches!(b, Boundary::BeforeDisplace(_)),
        move || {
            // Editors write a temporary file and rename it over the original.
            let victim = ["notes.txt", "old.txt", "run.sh"]
                .into_iter()
                .find(|n| root.join(n).exists())
                .unwrap();
            fs::write(root.join("editor.tmp"), b"EDITOR saved by rename\n").unwrap();
            fs::rename(root.join("editor.tmp"), root.join(victim)).unwrap();
        },
    );
    let (c, _, error, _, _) = raced(&f, tape);
    assert_conflict_preserved(&c, &f, &error, 0, &[b"EDITOR saved by rename\n"]);
    // The user's saved file is still at a user-visible destination.
    let at_destination = ["notes.txt", "old.txt", "run.sh"]
        .into_iter()
        .any(|n| fs::read(f.root.join(n)).is_ok_and(|b| b == b"EDITOR saved by rename\n"));
    assert!(at_destination);
}

#[test]
fn a_destination_recreated_between_displacement_and_publication_is_never_overwritten() {
    let f = fixture();
    let root = f.root.clone();
    let tape = Tape::on(
        |b| matches!(b, Boundary::AfterDisplace(_)),
        move || {
            for name in ["notes.txt", "old.txt", "run.sh"] {
                if !root.join(name).exists() {
                    fs::write(root.join(name), b"EDITOR recreated this\n").unwrap();
                    return;
                }
            }
            panic!("nothing was displaced");
        },
    );
    let (c, before, error, _, _) = raced(&f, tape);
    assert_conflict_preserved(&c, &f, &error, 0, &[b"EDITOR recreated this\n"]);
    // The displaced original is retained too, under its exact recovery name.
    let originals: Vec<_> = internal_files(&f.root)
        .into_iter()
        .filter(|p| p.ends_with(".orig"))
        .collect();
    assert_eq!(originals.len(), 1, "{originals:?}");
    let original = fs::read(f.root.join(&originals[0])).unwrap();
    assert!(
        before.values().any(|n| n.kind == 'f')
            && ["keep me\n", "to be deleted\n", "#!/bin/sh\necho hi\n"]
                .iter()
                .any(|t| t.as_bytes() == original)
    );
}

#[test]
fn a_write_through_an_open_descriptor_after_displacement_is_retained() {
    use std::io::Write;
    let f = fixture();
    // The editor keeps its file open across our displacement and writes afterwards.
    let handle = Arc::new(parking_lot::Mutex::new(
        fs::OpenOptions::new()
            .write(true)
            .open(f.root.join("old.txt"))
            .unwrap(),
    ));
    let writer = handle.clone();
    let tape = Tape::on(
        |b| matches!(b, Boundary::AfterDisplace(_)),
        move || {
            let mut file = writer.lock();
            file.write_all(b"LATE").unwrap();
            file.sync_all().unwrap();
        },
    );
    let (c, _, error, _, _) = raced(&f, tape);
    assert_conflict_preserved(&c, &f, &error, 0, &[]);
    // The late bytes live on in the retained slot or the restored file.
    let late = all_contents(&f.root)
        .into_iter()
        .any(|b| b.starts_with(b"LATE"));
    assert!(late, "the descriptor write was lost");
    drop(handle);
}

#[test]
fn a_late_descriptor_write_before_the_commit_is_caught_by_the_final_slot_check() {
    use std::io::Write;
    let f = fixture();
    let handle = Arc::new(parking_lot::Mutex::new(
        fs::OpenOptions::new()
            .write(true)
            .open(f.root.join("notes.txt"))
            .unwrap(),
    ));
    let writer = handle.clone();
    let tape = Tape::on(
        |b| matches!(b, Boundary::FinalInventory),
        move || {
            let mut file = writer.lock();
            file.write_all(b"after publication").unwrap();
            file.sync_all().unwrap();
        },
    );
    let (c, _, error, _, _) = raced(&f, tape);
    assert!(
        matches!(promotion(&error), PromotionError::Conflict(_)),
        "{error}"
    );
    assert!(c.state().task_history().entries().is_empty());
    assert!(
        all_contents(&f.root)
            .iter()
            .any(|b| b.windows(17).any(|w| w == b"after publication")),
        "bytes written through the stale descriptor were lost"
    );
    drop(handle);
}

#[test]
fn directory_swaps_and_links_halt_without_following() {
    // Parent directory replaced by a symlink to elsewhere right before displacement.
    let f = fixture();
    fs::create_dir_all(f.root.join("sub")).unwrap();
    fs::write(f.root.join("sub/inner.txt"), "inner\n").unwrap();
    let outside = f.temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("inner.txt"), "outside content\n").unwrap();
    let outside_before = snapshot(&outside);
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", |d| {
        fs::write(d.join("sub/inner.txt"), "agent\n").unwrap();
    });
    let root = f.root.clone();
    let hooks = Tape::on(
        |b| matches!(b, Boundary::BeforeDisplace(_)),
        move || {
            fs::rename(root.join("sub"), root.join("sub-moved")).unwrap();
            std::os::unix::fs::symlink(root.parent().unwrap().join("outside"), root.join("sub"))
                .unwrap();
        },
    );
    c.set_transaction_hooks(hooks.hooks());
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Conflict(_)),
        "{error}"
    );
    assert_eq!(
        snapshot(&outside),
        outside_before,
        "nothing was written through the link"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("sub-moved/inner.txt")).unwrap(),
        "inner\n"
    );
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn links_and_special_files_at_a_destination_are_refused_at_planning() {
    for variant in ["symlink", "fifo", "directory"] {
        let f = fixture();
        let mut c = Controller::open(&f.root, &f.paths).unwrap();
        let (_, captured, report) = validated(&f, &mut c, "edit", |d| {
            fs::write(d.join("added.txt"), "agent\n").unwrap();
        });
        let before_edit = snapshot(&f.root);
        // Between validation and publication the destination becomes something else.
        match variant {
            "symlink" => {
                std::os::unix::fs::symlink("/etc/passwd", f.root.join("added.txt")).unwrap()
            }
            "fifo" => {
                let made = std::process::Command::new("mkfifo")
                    .arg(f.root.join("added.txt"))
                    .status()
                    .unwrap();
                assert!(made.success());
            }
            _ => fs::create_dir(f.root.join("added.txt")).unwrap(),
        }
        let before = snapshot(&f.root);
        assert_ne!(before, before_edit);
        let error = c.complete_validated_task(&captured, &report).unwrap_err();
        // A link / special file makes the scan itself fail or the plan conflict; either
        // way nothing was written.
        assert!(
            matches!(
                &error,
                EngineError::Project(_)
                    | EngineError::Promotion(PromotionError::SourceChanged { .. })
                    | EngineError::Promotion(PromotionError::Plan(_))
            ),
            "{variant}: {error}"
        );
        assert_eq!(snapshot(&f.root), before, "{variant}");
    }
}

#[test]
fn disk_full_while_staging_rolls_back_with_no_trace() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let before = snapshot(&f.root);
    let hooks = Arc::new(move |b: &Boundary| {
        if matches!(b, Boundary::StageWritten(1)) {
            Err(studio_engine::Fault::Io(std::io::ErrorKind::StorageFull))
        } else {
            Ok(())
        }
    });
    c.set_transaction_hooks(hooks);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::RolledBack(_)),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
    assert_eq!(c.agent_task().unwrap().state(), TaskState::Failed);
    assert!(c.recovery_status().unresolved.is_empty());
    assert!(
        f.checkpoints(&c)
            .load(captured.candidate().revision())
            .is_ok()
    );
    // The same candidate can no longer be applied (the task failed) but a new task can.
    c.set_transaction_hooks(Arc::new(studio_engine::NoHooks));
    apply(&f, &mut c, "again", multi_file_edit);
}

#[test]
fn corrupt_or_missing_candidate_objects_fail_before_any_byte_changes() {
    for damage in ["missing", "corrupt"] {
        let f = fixture();
        let mut c = Controller::open(&f.root, &f.paths).unwrap();
        let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
        let target = captured
            .objects()
            .iter()
            .find(|o| o.path == "added.txt")
            .unwrap()
            .sha256
            .clone();
        let object = f.history(&c).join("objects").join(&target);
        match damage {
            "missing" => fs::remove_file(&object).unwrap(),
            _ => fs::write(&object, b"damaged").unwrap(),
        }
        let before = snapshot(&f.root);
        let error = c.complete_validated_task(&captured, &report).unwrap_err();
        assert!(
            matches!(error, EngineError::Project(_)),
            "{damage}: {error}"
        );
        assert_eq!(snapshot(&f.root), before, "{damage}");
        assert_eq!(
            c.agent_task().unwrap().state(),
            TaskState::Failed,
            "{damage}"
        );
        assert!(c.state().task_history().entries().is_empty());
    }
}

#[test]
fn an_object_that_decays_after_planning_rolls_back_cleanly() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let object = f.history(&c).join("objects").join(
        &captured
            .objects()
            .iter()
            .find(|o| o.path == "notes.txt")
            .unwrap()
            .sha256,
    );
    let before = snapshot(&f.root);
    let tape = Tape::on(
        |b| {
            matches!(
                b,
                Boundary::Append {
                    event: "intent",
                    after: true,
                    ..
                }
            )
        },
        move || fs::write(&object, b"bit rot").unwrap(),
    );
    c.set_transaction_hooks(tape.hooks());
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::RolledBack(_)),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
}

#[test]
fn a_journal_append_failure_suspends_mutation_and_the_next_open_recovers() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let before = snapshot(&f.root);
    let hooks = Arc::new(|b: &Boundary| match b {
        Boundary::Append {
            event: "progress",
            step: Some(studio_engine::edit_transaction::Step::Published),
            after: false,
            ..
        } => Err(studio_engine::Fault::Io(std::io::ErrorKind::StorageFull)),
        _ => Ok(()),
    });
    c.set_transaction_hooks(hooks);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Journal(_)),
        "{error}"
    );
    assert!(c.recovery_status().suspended.is_some());
    assert_ne!(
        snapshot(&f.root),
        before,
        "variants stay as they are for recovery"
    );
    // Everything is refused until reopen.
    assert!(c.ensure_mutable_for_test().is_err());
    drop(c);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.recovery_status().report.rolled_back.len(), 1);
    assert_eq!(
        without_internal(&snapshot(&f.root)),
        without_internal(&before)
    );
    assert!(internal_files(&f.root).is_empty());
    assert!(c.state().task_history().entries().is_empty());
}

trait EnsureMutable {
    fn ensure_mutable_for_test(&mut self) -> Result<(), EngineError>;
}
impl EnsureMutable for Controller {
    fn ensure_mutable_for_test(&mut self) -> Result<(), EngineError> {
        self.prepare_undo(None).map(drop)
    }
}

#[test]
fn conflicts_block_new_mutation_until_resolved_and_survive_reopen() {
    let f = fixture();
    let root = f.root.clone();
    let tape = Tape::on(
        |b| matches!(b, Boundary::AfterDisplace(_)),
        move || {
            for name in ["notes.txt", "old.txt", "run.sh"] {
                if !root.join(name).exists() {
                    fs::write(root.join(name), b"EDITOR recreated this\n").unwrap();
                    return;
                }
            }
        },
    );
    let (mut c, _, error, _, _) = raced(&f, tape);
    assert_conflict_preserved(&c, &f, &error, 0, &[b"EDITOR recreated this\n"]);
    let transaction = c.recovery_status().unresolved[0].transaction.clone();
    // Asset import, Apply and Undo are all refused while unresolved.
    let asset = f.temp.path().join("asset.txt");
    fs::write(&asset, "x").unwrap();
    assert!(matches!(
        promotion(&c.copy_asset(&asset).unwrap_err()),
        PromotionError::Unresolved(_)
    ));
    assert!(matches!(
        promotion(&c.prepare_undo(None).unwrap_err()),
        PromotionError::Unresolved(_)
    ));
    let UndoStatus::Unavailable { reason } = c.undo_status() else {
        panic!()
    };
    assert!(reason.contains("resolve"), "{reason}");
    // Reopening replays the journal: the same conflict, nothing silently replaced.
    let snapshot_before = snapshot(&f.root);
    drop(c);
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    assert_eq!(snapshot(&f.root), snapshot_before);
    assert!(c.recovery_notice.as_deref().unwrap().contains("conflict"));
    // Resolving unblocks, retains the variants and survives another reopen.
    c.resolve_conflict(&transaction, "kept the editor's version")
        .unwrap();
    assert!(c.recovery_status().unresolved.is_empty());
    assert_eq!(snapshot(&f.root), snapshot_before);
    drop(c);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert!(c.recovery_status().unresolved.is_empty());
}

#[test]
fn the_publication_primitive_is_proved_on_this_filesystem_before_apply_is_enabled() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    assert!(
        matches!(c.apply_gate(), studio_engine::ApplyGate::Ready(_)),
        "{:?}",
        c.apply_gate()
    );
    // The probe leaves nothing behind in the user's tree.
    assert!(internal_files(&f.root).is_empty());
}

#[test]
fn a_blocked_gate_refuses_every_publication_and_keeps_the_candidate_ready() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    c.override_apply_gate(Some(studio_engine::ApplyGate::Blocked(
        "no-clobber publication is not proven here".into(),
    )));
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let before = snapshot(&f.root);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::GateBlocked(_)),
        "{error}"
    );
    assert!(error.to_string().contains("not proven"));
    assert_eq!(snapshot(&f.root), before);
    // Auto-Apply could not run, but the validated candidate is still reviewable.
    assert_eq!(c.agent_task().unwrap().state(), TaskState::CandidateReady);
    let UndoStatus::Unavailable { reason } = c.undo_status() else {
        panic!("undo must be disabled with the gate");
    };
    assert!(reason.contains("blocked"), "{reason}");
    // Lifting the block (the real probe passes here) lets the same candidate apply.
    c.override_apply_gate(None);
    c.apply_candidate(&captured, &report).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 1);
}

/// The arbitration point sits after every refusal check and before the first durable
/// mutation: declining there changes nothing (no tree, no history, task still reviewable),
/// and the very same candidate applies afterwards. Undo has the same boundary.
#[test]
fn a_declined_publication_entry_changes_nothing_for_apply_and_undo() {
    let f = fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    c.set_review_policy(ReviewPolicy::ManualReview).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    assert!(matches!(
        c.complete_validated_task(&captured, &report).unwrap(),
        CompletionOutcome::AwaitingReview
    ));
    let before = snapshot(&f.root);
    let entered = std::cell::Cell::new(0);
    let error = c
        .apply_candidate_gated(&captured, &report, &|| {
            entered.set(entered.get() + 1);
            false
        })
        .unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Declined),
        "{error}"
    );
    assert_eq!(entered.get(), 1, "the boundary is crossed exactly once");
    assert_eq!(snapshot(&f.root), before);
    assert!(c.state().task_history().entries().is_empty());
    assert_eq!(c.agent_task().unwrap().state(), TaskState::CandidateReady);
    let applied = c.apply_candidate(&captured, &report).unwrap();
    assert_eq!(applied.record.kind, TransactionKind::Apply);

    let after_apply = snapshot(&f.root);
    let undo = c.prepare_undo(None).unwrap();
    let undo_report = passing_probe::passing_report(undo.captured());
    let error = c
        .undo_task_gated(&undo, &undo_report, &|| false)
        .unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Declined),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), after_apply);
    assert_eq!(c.state().task_history().entries().len(), 1);
    c.undo_task(&undo, &undo_report).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 2);
}

/// A link-then-unlink pair is never a substitute for an atomic rename on a live name (the
/// primitive-level regression is `studio-project/tests/publish.rs`, "link then unlink
/// loses an editor save between the two syscalls"): where only that exists, Apply and Undo
/// report `Blocked`, keep the candidate reviewable, and never touch a file.
#[test]
fn a_filesystem_with_only_the_link_pair_blocks_apply_and_undo_without_touching_files() {
    let f = fixture();
    let before = snapshot(&f.root);
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    c.force_publication_mechanism(Some(studio_engine::edit_transaction::NoClobber::Link));
    let studio_engine::ApplyGate::Blocked(reason) = c.apply_gate().clone() else {
        panic!("the link pair must not open the gate");
    };
    assert!(
        reason.contains("renameat2") && reason.contains("editor"),
        "{reason}"
    );
    let (_, captured, report) = validated(&f, &mut c, "via links", multi_file_edit);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::GateBlocked(_)),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
    assert!(internal_files(&f.root).is_empty());
    assert_eq!(c.agent_task().unwrap().state(), TaskState::CandidateReady);
    assert!(c.state().task_history().entries().is_empty());
    // Planning itself refuses the link mechanism even if a caller bypasses the gate.
    c.override_apply_gate(Some(studio_engine::ApplyGate::Ready(
        studio_engine::edit_transaction::NoClobber::Link,
    )));
    let error = c.apply_candidate(&captured, &report).unwrap_err();
    assert!(
        matches!(
            promotion(&error),
            PromotionError::Plan(studio_engine::PlanError::GateBlocked(_))
        ),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
    // With the atomic primitive an Apply goes through; Undo is blocked again on a link-only
    // filesystem and changes nothing.
    c.force_publication_mechanism(None);
    c.override_apply_gate(None);
    let applied = c.apply_candidate(&captured, &report).unwrap();
    let after_apply = snapshot(&f.root);
    c.force_publication_mechanism(Some(studio_engine::edit_transaction::NoClobber::Link));
    let error = match c.prepare_undo(Some(&applied.record.id)) {
        Err(error) => error,
        Ok(_) => panic!("Undo must be blocked"),
    };
    assert!(
        matches!(promotion(&error), PromotionError::GateBlocked(_)),
        "{error}"
    );
    assert!(matches!(
        c.undo_status(),
        UndoStatus::Unavailable { reason } if reason.contains("blocked")
    ));
    assert_eq!(snapshot(&f.root), after_apply);
    assert!(internal_files(&f.root).is_empty());
}
