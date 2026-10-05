//! Regressions for the Stage 3 review findings on publication: partial-stage ownership
//! (H1), root pathname fencing (H4), file <-> directory topology (H5) and exact
//! permission bits (M1).
#![cfg(target_os = "linux")]

#[path = "support/tx_fixture.rs"]
mod tx_fixture;

use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use studio_engine::{
    Boundary, CompletionOutcome, Controller, EngineError, PromotionError,
    candidate_validation::{CapturedCandidate, ValidationReport},
};
use tx_fixture::*;

fn promotion(error: &EngineError) -> &PromotionError {
    match error {
        EngineError::Promotion(p) => p,
        other => panic!("not a promotion error: {other}"),
    }
}

fn applied(
    c: &mut Controller,
    captured: &CapturedCandidate,
    report: &ValidationReport,
) -> studio_engine::Promotion {
    match c.complete_validated_task(captured, report).unwrap() {
        CompletionOutcome::Applied(p) => *p,
        other => panic!("expected an applied revision, got {other:?}"),
    }
}

fn undo(c: &mut Controller, target: &str) -> Result<studio_engine::Promotion, EngineError> {
    let preparation = c.prepare_undo(Some(target))?;
    let report = passing_probe::passing_report(preparation.captured());
    c.undo_task(&preparation, &report)
}

// ---- H1: partial staging is only collected when it is provably ours --------------------

/// A crash right after the first stage (`added.txt`, op 1) was created and its inode
/// journalled, before a single byte was written.
fn crashed_while_staging() -> (Fx, PathBuf) {
    let probe = Tape::recording();
    {
        let f = fixture();
        let mut c = open_retrying(&f.root, &f.paths).unwrap();
        let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
        c.set_transaction_hooks(probe.hooks());
        applied(&mut c, &captured, &report);
    }
    let at = probe
        .boundaries()
        .iter()
        .position(|b| *b == Boundary::StageCreated(1))
        .expect("a stage is created for op 1");
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    c.set_transaction_hooks(Tape::crashing_at(at).hooks());
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(is_crash(&error), "{error}");
    let tasks = f.task_journal(&c);
    drop(c);
    (f, tasks)
}

fn stage_of(f: &Fx) -> PathBuf {
    let name = internal_files(&f.root)
        .into_iter()
        .find(|n| n.ends_with("-1.stage"))
        .expect("the crashed stage exists");
    f.root.join(name)
}

#[test]
fn the_stage_inode_is_journalled_before_any_byte_is_written() {
    let (f, tasks) = crashed_while_staging();
    let journal = fs::read_to_string(tasks).unwrap();
    assert!(journal.contains("\"step\":\"stage_created\""), "{journal}");
    assert_eq!(fs::metadata(stage_of(&f)).unwrap().len(), 0);
}

#[test]
fn a_verified_prefix_of_our_own_stage_is_collected_on_recovery() {
    let (f, _) = crashed_while_staging();
    // The expected bytes are "added\n"; the crash left the first three of them.
    fs::write(stage_of(&f), "add").unwrap();
    let before = snapshot(&f.root);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.recovery_status().report.rolled_back.len(), 1);
    assert!(c.recovery_status().unresolved.is_empty());
    assert!(internal_files(&f.root).is_empty());
    assert_eq!(
        without_internal(&snapshot(&f.root)),
        without_internal(&before)
    );
}

#[test]
fn unrelated_shorter_bytes_in_the_stage_name_are_retained_as_a_conflict() {
    let (f, _) = crashed_while_staging();
    // Same inode, but bytes that are not a prefix of "added\n".
    fs::write(stage_of(&f), "zzz").unwrap();
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.recovery_status().report.rolled_back.len(), 0);
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    assert_eq!(fs::read(stage_of(&f)).unwrap(), b"zzz");
    let conflict = &c.recovery_status().unresolved[0];
    assert!(
        conflict
            .variants
            .iter()
            .any(|v| v.role == "stage" && v.size == Some(3)),
        "{conflict:?}"
    );
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn a_replaced_stage_inode_is_never_collected_even_when_its_bytes_look_right() {
    let (f, _) = crashed_while_staging();
    let stage = stage_of(&f);
    let original = fs::metadata(&stage).unwrap().ino();
    // Another file takes the stage's name; its bytes are a perfect prefix.
    let replacement = f.root.join(".someone-elses-temp");
    fs::write(&replacement, "add").unwrap();
    fs::rename(&replacement, &stage).unwrap();
    assert_ne!(fs::metadata(&stage).unwrap().ino(), original);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    assert_eq!(fs::read(&stage).unwrap(), b"add");
    assert!(c.state().task_history().entries().is_empty());
}

// ---- H4: the root pathname is fenced ---------------------------------------------------

#[test]
fn a_root_swapped_for_a_link_during_publication_is_a_conflict_never_a_commit() {
    type When = fn(&Boundary) -> bool;
    let cases: [(&str, When); 3] = [
        ("before publication", |b| {
            matches!(b, Boundary::BeforePublish(_))
        }),
        ("before the final inventory", |b| {
            *b == Boundary::FinalInventory
        }),
        ("before the commit", |b| *b == Boundary::InventoryVerified),
    ];
    for (label, when) in cases {
        let f = fixture();
        let mut c = open_retrying(&f.root, &f.paths).unwrap();
        let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
        let before = snapshot(&f.root);
        let root = f.root.clone();
        let moved = f.temp.path().join("video-moved");
        let target = moved.clone();
        // The project folder is renamed away and its old name becomes a link to it: the
        // final scan through the pathname would still describe the candidate.
        c.set_transaction_hooks(
            Tape::on(when, move || {
                fs::rename(&root, &target).unwrap();
                std::os::unix::fs::symlink(&target, &root).unwrap();
            })
            .hooks(),
        );
        let error = c.complete_validated_task(&captured, &report).unwrap_err();
        let reason = match promotion(&error) {
            PromotionError::RolledBack(reason) => reason.clone(),
            PromotionError::Conflict(report) => report.reason.clone(),
            other => panic!("{label}: expected a halted publication, got {other}"),
        };
        assert!(reason.contains("project folder"), "{label}: {reason}");
        assert_eq!(journal_commits(&f.task_journal(&c)), 0, "{label}");
        assert!(c.state().task_history().entries().is_empty(), "{label}");
        // Everything inside the moved folder is back to the byte.
        assert_eq!(
            without_internal(&snapshot(&moved)),
            without_internal(&before),
            "{label}"
        );
        assert!(internal_files(&moved).is_empty(), "{label}");
    }
}

// ---- H5: file <-> directory topology ---------------------------------------------------

/// `old.txt` (a file) becomes a directory holding a file.
fn file_to_dir(draft: &Path) {
    fs::remove_file(draft.join("old.txt")).unwrap();
    fs::create_dir(draft.join("old.txt")).unwrap();
    fs::write(draft.join("old.txt/inner.txt"), "inside\n").unwrap();
}

/// `clips/` (a 0750 directory with two files) becomes a file.
fn dir_to_file(draft: &Path) {
    fs::remove_file(draft.join("clips/a.txt")).unwrap();
    fs::remove_file(draft.join("clips/b.txt")).unwrap();
    fs::remove_dir(draft.join("clips")).unwrap();
    fs::write(draft.join("clips"), "now a file\n").unwrap();
}

fn with_media(root: &Path) {
    fs::create_dir(root.join("clips")).unwrap();
    fs::write(root.join("clips/a.txt"), "a\n").unwrap();
    fs::write(root.join("clips/b.txt"), "b\n").unwrap();
    fs::set_permissions(root.join("clips"), fs::Permissions::from_mode(0o750)).unwrap();
}

fn nothing(_: &Path) {}

#[derive(Clone, Copy)]
struct Case {
    prepare: fn(&Path),
    edit: fn(&Path),
    /// Interrupt the Undo of the edit instead of the edit itself.
    undo: bool,
}

struct Played {
    fx: Fx,
    tasks: PathBuf,
    /// The tree immediately before the transaction under test.
    before: BTreeMap<String, Node>,
    /// The source the transaction under test publishes.
    target: studio_project::SourceRevision,
    /// Committed revisions in the journal once the transaction committed.
    commits: usize,
}

fn play(case: Case, hooks: Option<Tape>) -> (Played, Controller, Result<(), EngineError>) {
    let fx = fixture();
    (case.prepare)(&fx.root);
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    let tasks = fx.task_journal(&c);
    let (_, captured, report) = validated(&fx, &mut c, "topology", case.edit);
    if !case.undo {
        let before = snapshot(&fx.root);
        let target = captured.candidate().revision().clone();
        if let Some(hooks) = hooks {
            c.set_transaction_hooks(hooks.hooks());
        }
        let result = c
            .complete_validated_task(&captured, &report)
            .map(|outcome| assert!(matches!(outcome, CompletionOutcome::Applied(_))));
        return (
            Played {
                fx,
                tasks,
                before,
                target,
                commits: 1,
            },
            c,
            result,
        );
    }
    let revision = applied(&mut c, &captured, &report);
    let before = snapshot(&fx.root);
    let preparation = c.prepare_undo(Some(&revision.record.id)).unwrap();
    let report = passing_probe::passing_report(preparation.captured());
    let target = preparation.captured().candidate().revision().clone();
    if let Some(hooks) = hooks {
        c.set_transaction_hooks(hooks.hooks());
    }
    let result = c.undo_task(&preparation, &report).map(drop);
    (
        Played {
            fx,
            tasks,
            before,
            target,
            commits: 2,
        },
        c,
        result,
    )
}

/// After a crash or failure at any point: reopening converges to exactly the committed
/// result or exactly the tree before, with nothing left over and no conflict.
fn assert_converged(p: &Played, label: &str) {
    let committed = journal_commits(&p.tasks) == p.commits;
    let c = open_retrying(&p.fx.root, &p.fx.paths)
        .unwrap_or_else(|e| panic!("{label}: reopen failed: {e}"));
    assert!(
        c.recovery_status().unresolved.is_empty(),
        "{label}: {:?}",
        c.recovery_status().unresolved
    );
    assert!(
        internal_files(&p.fx.root).is_empty(),
        "{label}: leftover {:?}",
        internal_files(&p.fx.root)
    );
    if committed {
        assert_eq!(c.state().source(), &p.target, "{label}");
    } else {
        assert_eq!(
            without_internal(&snapshot(&p.fx.root)),
            without_internal(&p.before),
            "{label}"
        );
        assert_eq!(
            c.state().task_history().entries().len(),
            p.commits - 1,
            "{label}"
        );
    }
}

fn matrix(case: Case, label: &str) {
    let probe = Tape::recording();
    let (played, c, result) = play(case, Some(probe.clone()));
    result.unwrap();
    drop((played, c));
    let count = probe.boundaries().len();
    assert!(count > 15, "{label}: only {count} boundaries");
    for n in 0..count {
        let (played, c, result) = play(case, Some(Tape::crashing_at(n)));
        assert!(
            is_crash(&result.unwrap_err()),
            "{label}: boundary {n} is reachable"
        );
        drop(c);
        assert_converged(&played, &format!("{label}: crash at {n}"));
    }
    for n in 0..count {
        let (played, c, _) = play(case, Some(Tape::failing_at(n, ErrorKind::Other)));
        drop(c);
        assert_converged(&played, &format!("{label}: I/O failure at {n}"));
    }
}

#[test]
fn a_file_becomes_a_directory_and_back_exactly() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let before = snapshot(&f.root);
    let (_, captured, report) = validated(&f, &mut c, "file to directory", file_to_dir);
    let revision = applied(&mut c, &captured, &report);
    assert!(f.root.join("old.txt").is_dir());
    assert_eq!(
        fs::read_to_string(f.root.join("old.txt/inner.txt")).unwrap(),
        "inside\n"
    );
    assert!(internal_files(&f.root).is_empty());
    assert_eq!(revision.record.created_dirs, ["old.txt"]);
    // The inverse: the draft must delete the file before the directory goes and a file
    // takes its name; publication removes the directory, then publishes the file.
    undo(&mut c, &revision.record.id).unwrap();
    assert!(internal_files(&f.root).is_empty());
    assert_eq!(
        without_internal(&snapshot(&f.root)),
        without_internal(&before)
    );
    assert_eq!(c.state().task_history().entries().len(), 2);
    drop(c);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 2);
}

#[test]
fn a_directory_becomes_a_file_and_back() {
    let f = fixture();
    with_media(&f.root);
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let before = snapshot(&f.root);
    let (_, captured, report) = validated(&f, &mut c, "directory to file", dir_to_file);
    let revision = applied(&mut c, &captured, &report);
    assert_eq!(
        fs::read_to_string(f.root.join("clips")).unwrap(),
        "now a file\n"
    );
    assert!(internal_files(&f.root).is_empty());
    undo(&mut c, &revision.record.id).unwrap();
    assert!(internal_files(&f.root).is_empty());
    assert_eq!(fs::read(f.root.join("clips/a.txt")).unwrap(), b"a\n");
    assert_eq!(fs::read(f.root.join("clips/b.txt")).unwrap(), b"b\n");
    // Only the directory's own mode is not part of a task revision.
    assert!(
        differences(&without_internal(&before), &snapshot(&f.root))
            .iter()
            .all(|p| p == "clips")
    );
}

#[test]
fn a_directory_holding_anything_the_revision_did_not_delete_is_never_emptied_by_force() {
    let f = fixture();
    with_media(&f.root);
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "directory to file", dir_to_file);
    // Something the inventory cannot see appears inside the directory meanwhile.
    fs::create_dir(f.root.join("clips/keep")).unwrap();
    let before = snapshot(&f.root);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::RolledBack(reason)
            if reason.contains("still holds")),
        "{error}"
    );
    assert_eq!(without_internal(&snapshot(&f.root)), before);
    assert_eq!(fs::read(f.root.join("clips/a.txt")).unwrap(), b"a\n");
}

#[test]
fn every_boundary_of_a_file_to_directory_apply_converges() {
    matrix(
        Case {
            prepare: nothing,
            edit: file_to_dir,
            undo: false,
        },
        "file -> directory Apply",
    );
}

#[test]
fn every_boundary_of_a_directory_to_file_apply_converges_and_restores_the_directory_mode() {
    matrix(
        Case {
            prepare: with_media,
            edit: dir_to_file,
            undo: false,
        },
        "directory -> file Apply",
    );
}

#[test]
fn every_boundary_of_the_undo_of_a_file_to_directory_edit_converges() {
    matrix(
        Case {
            prepare: nothing,
            edit: file_to_dir,
            undo: true,
        },
        "Undo of file -> directory",
    );
}

#[test]
fn every_boundary_of_the_undo_of_a_directory_to_file_edit_converges() {
    matrix(
        Case {
            prepare: with_media,
            edit: dir_to_file,
            undo: true,
        },
        "Undo of directory -> file",
    );
}

// ---- M1: exact permission bits ---------------------------------------------------------

fn mode_of(root: &Path, name: &str) -> u32 {
    fs::metadata(root.join(name)).unwrap().mode() & 0o7777
}

fn set_mode(root: &Path, name: &str, mode: u32) {
    fs::set_permissions(root.join(name), fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_content_only_replacement_keeps_the_exact_permission_bits() {
    let f = fixture();
    fs::write(f.root.join("m744.sh"), "#!/bin/sh\n").unwrap();
    set_mode(&f.root, "m744.sh", 0o744);
    // Executable only for "other": still an executable file for the inventory.
    fs::write(f.root.join("m641.txt"), "x\n").unwrap();
    set_mode(&f.root, "m641.txt", 0o641);
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "content only", |draft| {
        fs::write(draft.join("m744.sh"), "#!/bin/sh\necho new\n").unwrap();
        fs::write(draft.join("m641.txt"), "y\n").unwrap();
    });
    let revision = applied(&mut c, &captured, &report);
    assert_eq!(mode_of(&f.root, "m744.sh"), 0o744);
    assert_eq!(mode_of(&f.root, "m641.txt"), 0o641);
    // The record keeps both sides for an exact inverse.
    for name in ["m744.sh", "m641.txt"] {
        let delta = revision
            .record
            .changes
            .iter()
            .find(|d| d.path == name)
            .unwrap();
        assert_eq!(delta.before_mode, delta.after_mode, "{name}");
        assert!(delta.before_mode.is_some(), "{name}");
    }
    // And Undo puts the same bits back.
    undo(&mut c, &revision.record.id).unwrap();
    assert_eq!(mode_of(&f.root, "m744.sh"), 0o744);
    assert_eq!(mode_of(&f.root, "m641.txt"), 0o641);
    assert_eq!(
        fs::read_to_string(f.root.join("m744.sh")).unwrap(),
        "#!/bin/sh\n"
    );
}

#[test]
fn undo_restores_the_exact_bits_the_original_had_and_refuses_after_a_later_chmod() {
    let f = fixture();
    fs::write(f.root.join("odd.sh"), "#!/bin/sh\n").unwrap();
    // Executable for owner and other, plain read for group: dropping the executable
    // status yields 0640, from which a derived inverse could only guess 0750.
    set_mode(&f.root, "odd.sh", 0o741);
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let flip = |draft: &Path| {
        set_mode(draft, "odd.sh", 0o644);
    };
    let (_, captured, report) = validated(&f, &mut c, "drop the executable bit", flip);
    let revision = applied(&mut c, &captured, &report);
    assert_eq!(mode_of(&f.root, "odd.sh"), 0o640);
    let delta = revision
        .record
        .changes
        .iter()
        .find(|d| d.path == "odd.sh")
        .unwrap();
    assert_eq!(
        (delta.before_mode, delta.after_mode),
        (Some(0o741), Some(0o640))
    );
    undo(&mut c, &revision.record.id).unwrap();
    assert_eq!(mode_of(&f.root, "odd.sh"), 0o741, "the exact bits are back");

    // Apply again, then change the permissions behind the app's back (same bytes, still
    // not executable): the exact inverse is guarded and refuses.
    let (_, captured, report) = validated(&f, &mut c, "again", flip);
    let second = applied(&mut c, &captured, &report);
    set_mode(&f.root, "odd.sh", 0o600);
    let before = snapshot(&f.root);
    let error = match c.prepare_undo(Some(&second.record.id)) {
        Err(error) => error,
        Ok(_) => panic!("the permission change must be a conflict"),
    };
    let PromotionError::UndoConflict(conflicts) = promotion(&error) else {
        panic!("{error}");
    };
    assert!(
        conflicts
            .iter()
            .any(|c| c.path == "odd.sh" && c.reason.contains("permission bits")),
        "{conflicts:?}"
    );
    assert_eq!(snapshot(&f.root), before);
}
