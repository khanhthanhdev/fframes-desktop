//! Regressions for the Stage 3 review findings on history: bounded lifecycle snapshots
//! (H2), identity before replay (H3), journal-failure suspension (H7), authoritative task
//! history on recovery (H8), the non-mutating compatibility preflight (M2), the saved
//! history fence (M3), session-bound Undo (M4) and a committed outcome that survives a
//! bookkeeping failure (M5).
#![cfg(target_os = "linux")]

#[path = "support/tx_fixture.rs"]
mod tx_fixture;

use std::{
    collections::BTreeMap,
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    process::Command,
};
use studio_engine::{
    Boundary, CompletionOutcome, Controller, EngineError, PromotionError, ReviewPolicy, TaskState,
    UndoStatus,
    candidate_validation::{CapturedCandidate, ValidationReport},
    store::Store,
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

/// Every regular file below `dir` with its bytes: proves an operation changed nothing in
/// the app-local history.
fn tree_bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().display().to_string(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn project_history(f: &Fx, c: &Controller) -> PathBuf {
    f.history(c)
}

// ---- H2: task history never grows a lifecycle snapshot ---------------------------------

fn bulk_name(i: usize) -> String {
    // Long names make each delta heavy so a few thousand of them pass 1 MiB.
    format!("bulk/{i:0>230}")
}

#[test]
fn an_aggregate_history_beyond_one_mebibyte_still_opens_undoes_and_reopens() {
    const FILES: usize = 1000;
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();

    let create = |draft: &Path| {
        fs::create_dir_all(draft.join("bulk")).unwrap();
        for i in 0..FILES {
            fs::write(draft.join(bulk_name(i)), format!("v1 {i}\n")).unwrap();
        }
    };
    let (_, captured, report) = validated(&f, &mut c, "create the bulk files", create);
    applied(&mut c, &captured, &report);

    let rewrite = |draft: &Path| {
        for i in 0..FILES {
            fs::write(draft.join(bulk_name(i)), format!("v2 {i}\n")).unwrap();
        }
    };
    let (_, captured, report) = validated(&f, &mut c, "rewrite the bulk files", rewrite);
    applied(&mut c, &captured, &report);

    let remove = |draft: &Path| {
        for i in 0..FILES {
            fs::remove_file(draft.join(bulk_name(i))).unwrap();
        }
    };
    let (_, captured, report) = validated(&f, &mut c, "remove the bulk files", remove);
    let third = applied(&mut c, &captured, &report);

    // Each revision alone is admissible (its intent fits the task journal), the
    // aggregate is far beyond the lifecycle journal's 1 MiB entry limit.
    let aggregate = serde_json::to_vec(c.state().task_history().entries())
        .unwrap()
        .len();
    assert!(aggregate > 1 << 20, "history is only {aggregate} bytes");
    // Lifecycle snapshots do not carry it.
    let lifecycle = project_history(&f, &c).join("lifecycle.jsonl");
    let longest = fs::read_to_string(&lifecycle)
        .unwrap()
        .lines()
        .map(str::len)
        .max()
        .unwrap();
    assert!(longest < 64 * 1024, "a snapshot is {longest} bytes");

    // Undo works with the big history ...
    undo(&mut c, &third.record.id).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 4);
    assert!(f.root.join(bulk_name(0)).is_file());
    // ... and so do close and reopen.
    c.close().unwrap();
    drop(c);
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 4);
    assert!(c.recovery_status().unresolved.is_empty());
    assert!(c.recovery_status().suspended.is_none());
    let UndoStatus::Available { target, .. } = c.undo_status() else {
        panic!("Undo must stay available after reopening a large history");
    };
    assert_eq!(target, c.state().task_history().entries()[1].id);
    assert!(f.root.join(bulk_name(7)).is_file());
    // The projection and the in-memory history agree with the journal.
    let store = Store::open(&f.paths.database()).unwrap();
    assert_eq!(
        store
            .task_revisions(&c.project.manifest.project_id)
            .unwrap(),
        c.state().task_history().entries()
    );
    // A further Undo on the reopened controller still publishes.
    let second = c.state().task_history().entries()[1].id.clone();
    undo(&mut c, &second).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 5);
}

// ---- H3: identity is decided before any replay -----------------------------------------

fn index_of(boundary: Boundary) -> usize {
    let probe = Tape::recording();
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    c.set_transaction_hooks(probe.hooks());
    applied(&mut c, &captured, &report);
    probe
        .boundaries()
        .iter()
        .position(|b| *b == boundary)
        .expect("the boundary is reached")
}

/// A project whose multi-file Apply died with `notes.txt` displaced.
struct Crashed {
    f: Fx,
    tasks: PathBuf,
    history: PathBuf,
    tree: BTreeMap<String, Node>,
}

fn crashed_mid_apply() -> Crashed {
    let at = index_of(Boundary::AfterDisplace(3));
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    c.set_transaction_hooks(Tape::crashing_at(at).hooks());
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(is_crash(&error), "{error}");
    let tasks = f.task_journal(&c);
    let history = project_history(&f, &c);
    drop(c);
    let tree = snapshot(&f.root);
    Crashed {
        f,
        tasks,
        history,
        tree,
    }
}

#[test]
fn a_duplicate_project_id_is_refused_before_anything_is_replayed_or_written() {
    let crashed = crashed_mid_apply();
    let f = &crashed.f;
    // The project folder is copied (same project ID) while the Apply is unfinished.
    let copy = f.temp.path().join("copy");
    let status = Command::new("cp")
        .arg("-a")
        .arg(&f.root)
        .arg(&copy)
        .status()
        .unwrap();
    assert!(status.success());
    let copied_tree = snapshot(&copy);
    assert_eq!(copied_tree, crashed.tree);
    let history = tree_bytes(&f.paths.data);

    let error = Controller::open(&copy, &f.paths).err().unwrap();
    assert!(
        error.to_string().contains("Duplicate project ID"),
        "{error}"
    );
    // Neither the shared app-local history nor either project changed.
    assert_eq!(tree_bytes(&f.paths.data), history);
    assert_eq!(snapshot(&copy), copied_tree);
    assert_eq!(snapshot(&f.root), crashed.tree);
    assert!(crashed.tasks.exists());

    // The original still finds its pending edit and rolls it back cleanly.
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.recovery_status().report.rolled_back.len(), 1);
    assert!(c.recovery_status().report.conflicts.is_empty());
    assert!(c.recovery_status().unresolved.is_empty());
    assert!(internal_files(&f.root).is_empty());
}

// ---- H7: a lifecycle journal failure suspends mutation ---------------------------------

#[test]
fn a_failed_lifecycle_append_suspends_every_source_changing_entry_point() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let first = {
        let (_, captured, report) = validated(&f, &mut c, "first", multi_file_edit);
        applied(&mut c, &captured, &report)
    };
    assert!(c.recovery_status().suspended.is_none());
    // The disk fills up: the next lifecycle append really fails (ENOSPC).
    c.simulate_lifecycle_full_disk().unwrap();
    c.set_review_policy(ReviewPolicy::ManualReview).unwrap_err();
    let reason = c
        .recovery_status()
        .suspended
        .clone()
        .expect("a failed durable journal suspends mutation");
    assert!(reason.contains("lifecycle journal"), "{reason}");
    assert!(
        c.recovery_notice
            .as_deref()
            .is_some_and(|n| n.contains("suspended"))
    );

    // Space coming back does not lift it without a reopen: every entry point refuses.
    let before = snapshot(&f.root);
    let error = match c.prepare_undo(Some(&first.record.id)) {
        Err(error) => error,
        Ok(_) => panic!("Undo must refuse"),
    };
    assert!(
        matches!(promotion(&error), PromotionError::Journal(_)),
        "{error}"
    );
    let outside = f.temp.path().join("outside.txt");
    fs::write(&outside, "asset").unwrap();
    let error = c.copy_asset(&outside).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Journal(_)),
        "{error}"
    );
    let (_, captured, report) = validated(&f, &mut c, "second", |d| {
        fs::write(d.join("later.txt"), "later\n").unwrap();
    });
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::Journal(_)),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
    assert!(matches!(
        c.undo_status(),
        UndoStatus::Unavailable { reason } if reason.contains("History storage failed")
    ));
    drop(c);
    // Reopening recovers: the task journal is intact and mutation works again.
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    assert!(c.recovery_status().suspended.is_none());
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert!(matches!(c.undo_status(), UndoStatus::Available { .. }));
}

#[test]
fn a_lifecycle_failure_during_apply_keeps_the_acceptance_and_suspends_what_follows() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    c.simulate_lifecycle_full_disk().unwrap();
    // The durable task commit is what counts: the promotion is returned.
    let promotion = applied(&mut c, &captured, &report);
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert!(c.recovery_status().suspended.is_some());
    assert!(
        c.prepare_undo(Some(&promotion.record.id)).is_err(),
        "no further source change while lifecycle recovery is unresolved"
    );
    drop(c);
    let c = open_retrying(&f.root, &f.paths).unwrap();
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert!(c.recovery_status().suspended.is_none());
}

#[test]
fn a_stale_sqlite_projection_alone_stays_a_note() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let db = rusqlite::Connection::open(f.paths.database()).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_history_update BEFORE UPDATE ON projects BEGIN
            SELECT RAISE(FAIL, 'injected history write failure'); END;",
    )
    .unwrap();
    let error = c.set_review_policy(ReviewPolicy::ManualReview).unwrap_err();
    assert!(
        error.to_string().contains("injected history write failure"),
        "{error}"
    );
    assert!(
        c.recovery_status().suspended.is_none(),
        "the journals are intact"
    );
    assert!(c.recovery_notice.is_some());
    // Source changes keep working: only the SQLite projection is stale.
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    // (The failed write above still changed the policy in memory and the journal.)
    assert!(matches!(
        c.complete_validated_task(&captured, &report).unwrap(),
        CompletionOutcome::AwaitingReview
    ));
    let promotion = c.apply_candidate(&captured, &report).unwrap();
    assert_eq!(c.state().source(), &promotion.record.published);
    assert!(c.recovery_status().suspended.is_none());
    assert_eq!(journal_commits(&f.task_journal(&c)), 1);
}

// ---- H8: the task journal is authoritative after every substitution --------------------

#[test]
fn an_old_pending_job_intent_never_hides_a_later_task_commit() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    // A checkpoint job that wrote its lifecycle intent and then "died".
    let died = catch_unwind(AssertUnwindSafe(|| {
        let _ = c.checkpoint_observed(|stage| {
            if stage == "intent" {
                panic!("simulated process death after the job intent");
            }
        });
    }));
    assert!(died.is_err());
    // The lifecycle journal then stops accepting writes, yet a task commits durably.
    c.simulate_lifecycle_full_disk().unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let revision = applied(&mut c, &captured, &report);
    drop(c);

    for round in 0..3 {
        let mut c = open_retrying(&f.root, &f.paths).unwrap();
        assert_eq!(
            c.state().task_history().entries().len(),
            1,
            "round {round}: the commit must survive the stale pending record"
        );
        let UndoStatus::Available { target, .. } = c.undo_status() else {
            panic!("round {round}: Undo must see the committed revision");
        };
        assert_eq!(target, revision.record.id, "round {round}");
        let store = Store::open(&f.paths.database()).unwrap();
        assert_eq!(
            store
                .task_revisions(&c.project.manifest.project_id)
                .unwrap(),
            c.state().task_history().entries(),
            "round {round}"
        );
        assert_eq!(
            c.state().source(),
            &revision.record.published,
            "round {round}"
        );
        if round == 2 {
            // And it really can be undone.
            undo(&mut c, &revision.record.id).unwrap();
        }
    }
}

// ---- M2: compatibility is decided before any mutation ----------------------------------

#[test]
fn an_unsupported_task_journal_changes_no_history_file_even_with_an_old_database_and_a_torn_tail() {
    let f = fixture();
    let (history, tasks) = {
        let mut c = open_retrying(&f.root, &f.paths).unwrap();
        let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
        applied(&mut c, &captured, &report);
        (project_history(&f, &c), f.task_journal(&c))
    };
    // An installation from before task transactions: version-1 database ...
    rusqlite::Connection::open(f.paths.database())
        .unwrap()
        .execute_batch("DROP TABLE task_revisions; PRAGMA user_version=1;")
        .unwrap();
    // ... a torn lifecycle tail that a normal open would quarantine ...
    let lifecycle = history.join("lifecycle.jsonl");
    let mut torn = fs::read(&lifecycle).unwrap();
    torn.extend_from_slice(b"{\"version\":1,\"sequence\":");
    fs::write(&lifecycle, &torn).unwrap();
    // ... and a task journal written by a newer Studio.
    let mut future = fs::read(&tasks).unwrap();
    future.extend_from_slice(b"{\"format\":3,\"sequence\":99}\n");
    fs::write(&tasks, &future).unwrap();

    let tree = snapshot(&f.root);
    let everything = tree_bytes(&f.paths.data);
    let error = Controller::open(&f.root, &f.paths).err().unwrap();
    assert!(
        matches!(error, EngineError::NewerFormat { found: 3, .. }),
        "{error}"
    );
    // No migration (so no backup), no quarantine file, no new byte anywhere.
    assert_eq!(tree_bytes(&f.paths.data), everything);
    assert_eq!(snapshot(&f.root), tree);
    let names: Vec<_> = fs::read_dir(&f.paths.data)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("backup-")), "{names:?}");
    assert_eq!(
        rusqlite::Connection::open(f.paths.database())
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn a_damaged_interior_lifecycle_journal_is_refused_before_replay_touches_anything() {
    let crashed = crashed_mid_apply();
    let f = &crashed.f;
    // An unfinished transaction exists; the lifecycle journal is damaged inside.
    let lifecycle = crashed.history.join("lifecycle.jsonl");
    let mut damaged = b"{garbage}\n".to_vec();
    damaged.extend_from_slice(&fs::read(&lifecycle).unwrap());
    fs::write(&lifecycle, &damaged).unwrap();
    let everything = tree_bytes(&f.paths.data);
    assert!(Controller::open(&f.root, &f.paths).is_err());
    assert_eq!(tree_bytes(&f.paths.data), everything);
    assert_eq!(snapshot(&f.root), crashed.tree, "nothing was replayed");
    assert!(crashed.tasks.exists());
}

// ---- M3: the saved-history fence -------------------------------------------------------

#[test]
fn a_checkpoint_taken_while_a_candidate_validates_fences_apply() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    // Dirty source B over saved checkpoint A.
    fs::write(f.root.join("notes.txt"), "dirty import B\n").unwrap();
    c.reconcile().unwrap();
    let saved_a = c.state().accepted().clone();
    let (_, captured, report) = validated(&f, &mut c, "edit", |d| {
        fs::write(d.join("added.txt"), "agent\n").unwrap();
    });
    // The user saves a checkpoint of B while the candidate is being validated: the
    // source and the task-history head are unchanged, the saved history is not.
    c.checkpoint().unwrap();
    assert_ne!(c.state().accepted(), &saved_a);
    let before = snapshot(&f.root);
    let error = c.complete_validated_task(&captured, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::SavedHistoryChanged),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
    assert_eq!(c.agent_task().unwrap().state(), TaskState::Conflict);
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn a_checkpoint_between_preparing_and_publishing_undo_fences_the_undo() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let revision = applied(&mut c, &captured, &report);
    let preparation = c.prepare_undo(Some(&revision.record.id)).unwrap();
    let report = passing_probe::passing_report(preparation.captured());
    c.checkpoint().unwrap();
    let before = snapshot(&f.root);
    let error = c.undo_task(&preparation, &report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::SavedHistoryChanged),
        "{error}"
    );
    assert_eq!(snapshot(&f.root), before);
    assert_eq!(c.state().task_history().entries().len(), 1);
    // A fresh preparation sees the new saved history and publishes.
    undo(&mut c, &revision.record.id).unwrap();
}

// ---- M4: Undo preparations are bound to their session ----------------------------------

#[test]
fn an_undo_prepared_in_a_closed_session_or_another_controller_never_publishes() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    let revision = applied(&mut c, &captured, &report);
    let stale = c.prepare_undo(Some(&revision.record.id)).unwrap();
    let stale_report = passing_probe::passing_report(stale.captured());
    c.close().unwrap();
    drop(c);

    // The same project reopened: new session, source and history exactly as before.
    let mut reopened = open_retrying(&f.root, &f.paths).unwrap();
    let tasks = f.task_journal(&reopened);
    let journal = fs::read(&tasks).unwrap();
    let tree = snapshot(&f.root);
    let reports = f.history(&reopened).join("reports");
    let stored = fs::read_dir(&reports).unwrap().count();
    let error = reopened.undo_task(&stale, &stale_report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::StalePreparation(_)),
        "{error}"
    );
    assert_eq!(fs::read(&tasks).unwrap(), journal, "no intent was written");
    assert_eq!(snapshot(&f.root), tree);
    assert_eq!(
        fs::read_dir(&reports).unwrap().count(),
        stored,
        "no report was stored"
    );

    // Another project's controller refuses it as well.
    let other = fixture();
    let mut foreign = Controller::open(&other.root, &other.paths).unwrap();
    let before = snapshot(&other.root);
    let error = foreign.undo_task(&stale, &stale_report).unwrap_err();
    assert!(
        matches!(promotion(&error), PromotionError::StalePreparation(_)),
        "{error}"
    );
    assert_eq!(snapshot(&other.root), before);
    // A fresh preparation in the live session still publishes.
    undo(&mut reopened, &revision.record.id).unwrap();
}

// ---- M5: a committed promotion survives a bookkeeping failure --------------------------

#[test]
fn a_draft_marker_failure_after_the_commit_still_returns_the_committed_promotion() {
    let f = fixture();
    let mut c = open_retrying(&f.root, &f.paths).unwrap();
    let (_, captured, report) = validated(&f, &mut c, "edit", multi_file_edit);
    // The draft ownership marker can no longer be written.
    let marker = f.paths.agent_draft_state(&c.project.manifest.project_id);
    fs::remove_file(&marker).unwrap();
    fs::create_dir(&marker).unwrap();
    let promotion = applied(&mut c, &captured, &report);
    assert!(
        promotion
            .notes
            .iter()
            .any(|n| n.contains("draft bookkeeping")),
        "{:?}",
        promotion.notes
    );
    // The accepted revision is fully usable by the caller.
    assert!(promotion.authorization.is_some());
    assert_eq!(c.state().source(), captured.candidate().revision());
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert_eq!(journal_commits(&f.task_journal(&c)), 1);
    // Recovery is required (reopen): further mutation is suspended meanwhile.
    assert!(c.recovery_status().suspended.is_some());
    assert!(c.prepare_undo(Some(&promotion.record.id)).is_err());
}
