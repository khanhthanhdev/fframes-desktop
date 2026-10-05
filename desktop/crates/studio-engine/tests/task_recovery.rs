//! Crash and corruption recovery of file-set transactions: every durable boundary,
//! idempotent replay, conflicted recovery, journal damage and projection loss.
#![cfg(target_os = "linux")]

#[path = "support/tx_fixture.rs"]
mod tx_fixture;

use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};
use studio_engine::{
    Boundary, CompletionOutcome, Controller, EngineError, NoHooks, PromotionError,
    app_paths::AppPaths,
    diagnostics::{TaskJournalDiagnostics, diagnose_history},
    store::Store,
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

struct Run {
    fx: Fx,
    before: BTreeMap<String, Node>,
    candidate: studio_project::SourceRevision,
    saved: studio_project::SourceRevision,
    tasks: PathBuf,
}

/// Builds a fixture and a validated candidate, then applies it under `hooks`.
fn run(hooks: Option<Tape>) -> (Run, Controller, Result<(), EngineError>) {
    let fx = fixture();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    let saved = c.state().accepted().clone();
    let (_, captured, report) = validated(&fx, &mut c, "multi-file edit", multi_file_edit);
    let before = snapshot(&fx.root);
    if let Some(hooks) = hooks {
        c.set_transaction_hooks(hooks.hooks());
    }
    let result = c
        .complete_validated_task(&captured, &report)
        .map(|outcome| {
            assert!(matches!(outcome, CompletionOutcome::Applied(_)));
        });
    let tasks = fx.task_journal(&c);
    let candidate = captured.candidate().revision().clone();
    (
        Run {
            fx,
            before,
            candidate,
            saved,
            tasks,
        },
        c,
        result,
    )
}

/// Asserts the invariants that must hold after reopening, whatever boundary was hit.
fn assert_recovered(run: &Run, label: &str) -> Controller {
    let committed = journal_commits(&run.tasks) == 1;
    let c = open_retrying(&run.fx.root, &run.fx.paths)
        .unwrap_or_else(|e| panic!("{label}: reopen failed: {e}"));
    // Pending agent work never resumes by itself.
    assert!(c.agent_task().is_none(), "{label}");
    let after = snapshot(&run.fx.root);
    assert!(
        internal_files(&run.fx.root).is_empty(),
        "{label}: leftover {:?}",
        internal_files(&run.fx.root)
    );
    assert!(c.recovery_status().unresolved.is_empty(), "{label}");
    assert_eq!(
        c.state().accepted(),
        &run.saved,
        "{label}: checkpoint moved"
    );
    // Bytes, permissions and Git are unchanged outside the task in both outcomes.
    assert_eq!(
        outside_task(&run.before, &after, TOUCHED),
        Vec::<String>::new(),
        "{label}"
    );
    if committed {
        assert_eq!(c.state().source(), &run.candidate, "{label}");
        assert_eq!(c.state().task_history().entries().len(), 1, "{label}");
        assert_eq!(
            fs::read_to_string(run.fx.root.join("notes.txt")).unwrap(),
            "edited by the agent\n",
            "{label}"
        );
    } else {
        // Acceptance never advances without a durable commit, and every touched file is
        // back to its exact bytes and permissions.
        assert_eq!(
            without_internal(&after),
            without_internal(&run.before),
            "{label}"
        );
        assert!(c.state().task_history().entries().is_empty(), "{label}");
        assert_ne!(c.state().source(), &run.candidate, "{label}");
    }
    // The projection always matches the journal after open.
    let store = Store::open(&run.fx.paths.database()).unwrap();
    assert_eq!(
        store
            .task_revisions(&c.project.manifest.project_id)
            .unwrap(),
        c.state().task_history().entries(),
        "{label}"
    );
    c
}

fn assert_idempotent(run: &Run, label: &str) {
    let journal = fs::read(&run.tasks).unwrap();
    let tree = snapshot(&run.fx.root);
    for round in 0..2 {
        let c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
        assert!(
            c.recovery_status().report.rolled_back.is_empty()
                && c.recovery_status().report.cleaned.is_empty()
                && c.recovery_status().report.conflicts.is_empty(),
            "{label}: round {round} recovered again: {:?}",
            c.recovery_status().report
        );
        assert_eq!(
            fs::read(&run.tasks).unwrap(),
            journal,
            "{label}: journal grew"
        );
        assert_eq!(snapshot(&run.fx.root), tree, "{label}: tree changed");
    }
}

fn boundary_count() -> usize {
    let tape = Tape::recording();
    let (_, _c, result) = run(Some(tape.clone()));
    result.unwrap();
    tape.boundaries().len()
}

#[test]
fn the_boundary_sequence_covers_every_durable_step() {
    let tape = Tape::recording();
    let (_, _c, result) = run(Some(tape.clone()));
    result.unwrap();
    let seen = tape.boundaries();
    let has = |f: &dyn Fn(&Boundary) -> bool| seen.iter().any(f);
    assert!(has(&|b| matches!(
        b,
        Boundary::Append {
            event: "intent",
            after: false,
            ..
        }
    )));
    assert!(has(&|b| matches!(
        b,
        Boundary::Append {
            event: "intent",
            after: true,
            ..
        }
    )));
    // Ops: 0 deletes old.txt, 1-2 create, 3-4 replace (deletes first, then creates).
    for op in 1..5 {
        assert!(has(&|b| *b == Boundary::StageCreated(op)), "stage {op}");
        assert!(has(&|b| *b == Boundary::StageWritten(op)));
        assert!(has(&|b| *b == Boundary::StageSynced(op)));
        assert!(has(&|b| *b == Boundary::BeforePublish(op)));
    }
    assert!(!has(&|b| *b == Boundary::StageCreated(0)));
    // The delete and the two replacements displace; creates do not.
    for op in [0, 3, 4] {
        assert!(has(&|b| *b == Boundary::BeforeDisplace(op)));
        assert!(has(&|b| *b == Boundary::AfterDisplace(op)));
        assert!(has(&|b| *b == Boundary::DisplacedVerified(op)));
    }
    for op in 1..5 {
        assert!(has(&|b| *b == Boundary::AfterPublish(op)));
        assert!(has(&|b| *b == Boundary::PublishVerified(op)));
    }
    assert!(has(&|b| matches!(
        b,
        Boundary::Append {
            event: "progress",
            step: Some(studio_engine::edit_transaction::Step::Published),
            after: true,
            ..
        }
    )));
    assert!(has(&|b| *b == Boundary::FinalInventory));
    assert!(has(&|b| *b == Boundary::InventoryVerified));
    assert!(has(&|b| matches!(
        b,
        Boundary::Append {
            event: "commit",
            after: false,
            ..
        }
    )));
    assert!(has(&|b| matches!(
        b,
        Boundary::Append {
            event: "commit",
            after: true,
            ..
        }
    )));
    assert!(has(&|b| *b == Boundary::Database { after: false }));
    assert!(has(&|b| *b == Boundary::Database { after: true }));
    assert!(has(&|b| *b == Boundary::Projection { after: false }));
    assert!(has(&|b| *b == Boundary::Projection { after: true }));
    assert!(has(&|b| matches!(b, Boundary::Cleanup(_))));
}

#[test]
fn a_process_death_at_every_durable_boundary_recovers_without_loss() {
    let total = boundary_count();
    assert!(total > 60, "unexpectedly few boundaries: {total}");
    for k in 0..total {
        let (run, c, result) = run(Some(Tape::crashing_at(k)));
        let error = result.unwrap_err();
        assert!(is_crash(&error), "boundary {k}: {error}");
        // Acceptance never advances in memory without the durable commit.
        let committed = journal_commits(&run.tasks);
        assert!(
            c.state().task_history().entries().len() <= committed,
            "boundary {k}: memory ahead of the journal"
        );
        drop(c);
        let label = format!("crash at boundary {k}");
        drop(assert_recovered(&run, &label));
        assert_idempotent(&run, &label);
    }
}

#[test]
fn an_io_failure_at_every_durable_boundary_ends_cleanly_or_suspends_for_recovery() {
    let total = boundary_count();
    for k in 0..total {
        let (run, c, result) = run(Some(Tape::failing_at(k, ErrorKind::StorageFull)));
        let label = format!("I/O failure at boundary {k}");
        let committed = journal_commits(&run.tasks);
        match &result {
            Ok(()) => assert_eq!(committed, 1, "{label}"),
            Err(EngineError::Promotion(
                PromotionError::RolledBack(_) | PromotionError::Journal(_),
            )) => (),
            Err(other) => panic!("{label}: {other}"),
        }
        assert!(
            c.state().task_history().entries().len() <= committed,
            "{label}: memory ahead of the journal"
        );
        if committed == 0 {
            // Nothing was accepted: the tree is already back (or awaiting recovery).
            assert!(c.state().task_history().entries().is_empty(), "{label}");
        }
        drop(c);
        drop(assert_recovered(&run, &label));
        assert_idempotent(&run, &label);
    }
}

/// A crash in the middle of an Apply that published every file, to exercise a full
/// rollback during recovery.
fn crash_after_all_published() -> Run {
    let probe = Tape::recording();
    let (_, _c, result) = run(Some(probe.clone()));
    result.unwrap();
    let at = probe
        .boundaries()
        .iter()
        .position(|b| *b == Boundary::FinalInventory)
        .unwrap();
    let (run, c, result) = self::run(Some(Tape::crashing_at(at)));
    assert!(is_crash(&result.unwrap_err()));
    drop(c);
    run
}

#[test]
fn a_process_death_during_recovery_itself_still_converges() {
    // Count the boundaries of an uninterrupted recovery.
    let recording = Tape::recording();
    let reference = crash_after_all_published();
    drop(
        open_with_hooks_retrying(&reference.fx.root, &reference.fx.paths, recording.hooks())
            .unwrap(),
    );
    let recovery_total = recording.boundaries().len();
    assert!(recovery_total > 15, "{recovery_total}");
    for j in 0..recovery_total {
        let run = crash_after_all_published();
        let error =
            open_with_hooks_retrying(&run.fx.root, &run.fx.paths, Tape::crashing_at(j).hooks())
                .err()
                .unwrap_or_else(|| panic!("recovery boundary {j} did not crash"));
        assert!(is_crash(&error), "recovery boundary {j}: {error}");
        let label = format!("crash inside recovery at {j}");
        drop(assert_recovered(&run, &label));
        assert_idempotent(&run, &label);
    }
}

#[test]
fn recovery_never_overwrites_bytes_an_editor_wrote_while_the_app_was_down() {
    // Crash with every file published, then the editor saves one of them.
    let run = crash_after_all_published();
    fs::write(
        run.fx.root.join("notes.txt"),
        "EDITOR saved while the app was down\n",
    )
    .unwrap();
    let editor_notes = snapshot(&run.fx.root);
    let mut c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    assert_eq!(c.recovery_status().report.rolled_back.len(), 0);
    let conflict = c.recovery_status().unresolved[0].clone();
    assert!(conflict.ops.iter().any(|op| op.path == "notes.txt"));
    // The editor's bytes are untouched, and the displaced original of that file is kept.
    assert_eq!(
        fs::read_to_string(run.fx.root.join("notes.txt")).unwrap(),
        "EDITOR saved while the app was down\n"
    );
    assert!(
        survives(&run.fx.root, b"keep me\n"),
        "displaced original was lost"
    );
    assert!(conflict.variants.iter().any(|v| v.role == "original"));
    // Every other operation rolled back; only the conflicted file differs from the start.
    let after = snapshot(&run.fx.root);
    let differing: Vec<_> = differences(&without_internal(&run.before), &without_internal(&after));
    assert_eq!(differing, ["notes.txt"]);
    assert_ne!(editor_notes, run.before);
    // Mutation is blocked until resolved, across reopen, and replay stays idempotent.
    assert!(c.prepare_undo(None).is_err());
    let journal = fs::read(&run.tasks).unwrap();
    let transaction = conflict.transaction.clone();
    drop(c);
    let mut c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    assert_eq!(fs::read(&run.tasks).unwrap(), journal);
    assert_eq!(snapshot(&run.fx.root), after);
    c.resolve_conflict(&transaction, "kept the editor's version")
        .unwrap();
    assert!(c.recovery_status().unresolved.is_empty());
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn a_recreated_destination_during_a_crashed_displacement_halts_as_a_conflict() {
    let probe = Tape::recording();
    let (_, _c, result) = run(Some(probe.clone()));
    result.unwrap();
    let at = probe
        .boundaries()
        .iter()
        .position(|b| *b == Boundary::AfterDisplace(3))
        .unwrap();
    let (run, c, result) = run(Some(Tape::crashing_at(at)));
    assert!(is_crash(&result.unwrap_err()));
    drop(c);
    // op 3 displaced notes.txt; an editor recreates it while the app is down.
    assert!(!run.fx.root.join("notes.txt").exists());
    fs::write(run.fx.root.join("notes.txt"), "EDITOR recreated it\n").unwrap();
    let c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert_eq!(c.recovery_status().unresolved.len(), 1);
    assert_eq!(
        fs::read_to_string(run.fx.root.join("notes.txt")).unwrap(),
        "EDITOR recreated it\n"
    );
    assert!(survives(&run.fx.root, b"keep me\n"));
    // Operations unaffected by the editor were rolled back.
    assert!(run.fx.root.join("old.txt").exists());
    assert!(!run.fx.root.join("added.txt").exists());
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn the_projection_is_rebuilt_from_the_journal_when_lost_or_stale() {
    // A crash after the durable commit but before the database was updated.
    let probe = Tape::recording();
    let (_, _c, result) = run(Some(probe.clone()));
    result.unwrap();
    let at = probe
        .boundaries()
        .iter()
        .position(|b| *b == Boundary::Database { after: false })
        .unwrap();
    let (run, c, result) = run(Some(Tape::crashing_at(at)));
    assert!(is_crash(&result.unwrap_err()));
    drop(c);
    assert_eq!(journal_commits(&run.tasks), 1);
    let c = assert_recovered(&run, "stale projection");
    assert!(c.recovery_status().projection_rebuilt);
    assert_eq!(c.state().task_history().entries().len(), 1);
    drop(c);
    assert_idempotent(&run, "stale projection");

    // Missing database file entirely, and a dropped projection table.
    let db = run.fx.paths.database();
    fs::remove_file(&db).unwrap();
    let c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert!(c.recovery_status().projection_rebuilt);
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert_eq!(c.state().source(), &run.candidate);
    drop(c);
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE task_revisions")
        .unwrap();
    let c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert!(c.recovery_status().projection_rebuilt);
    assert_eq!(
        Store::open(&db)
            .unwrap()
            .task_revisions(&c.project.manifest.project_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn journal_tail_truncation_and_interior_corruption_are_handled_without_data_loss() {
    let (run, c, result) = run(None);
    result.unwrap();
    drop(c);
    let intact = fs::read(&run.tasks).unwrap();
    // A torn final append is quarantined; committed history is untouched.
    let mut torn = intact.clone();
    torn.extend_from_slice(b"{\"format\":2,\"sequence\":99,\"transa");
    fs::write(&run.tasks, &torn).unwrap();
    let c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert!(
        c.recovery_notice
            .as_deref()
            .unwrap()
            .contains("Truncated task journal tail")
    );
    assert_eq!(c.state().task_history().entries().len(), 1);
    assert_eq!(fs::read(&run.tasks).unwrap(), intact);
    drop(c);
    // Interior corruption refuses to open and leaves every byte in place.
    let mut damaged = intact.clone();
    damaged[5] = b'#';
    fs::write(&run.tasks, &damaged).unwrap();
    let tree = snapshot(&run.fx.root);
    let error = open_retrying(&run.fx.root, &run.fx.paths).err().unwrap();
    assert!(error.to_string().contains("malformed interior"), "{error}");
    assert_eq!(fs::read(&run.tasks).unwrap(), damaged);
    assert_eq!(snapshot(&run.fx.root), tree);
    // A torn Commit line means the commit never became durable: the unfinished
    // transaction is rolled back where its files still verify, or kept as a conflict.
    let lines: Vec<&[u8]> = intact.split_inclusive(|b| *b == b'\n').collect();
    let commit = lines
        .iter()
        .position(|l| String::from_utf8_lossy(l).contains("\"Commit\""))
        .unwrap();
    let mut cut = Vec::new();
    for line in &lines[..commit] {
        cut.extend_from_slice(line);
    }
    cut.extend_from_slice(&lines[commit][..lines[commit].len() / 2]);
    fs::write(&run.tasks, &cut).unwrap();
    let c = open_retrying(&run.fx.root, &run.fx.paths).unwrap();
    assert!(c.state().task_history().entries().is_empty());
    assert_eq!(
        c.recovery_status().report.rolled_back.len() + c.recovery_status().unresolved.len(),
        1
    );
}

#[test]
fn a_newer_task_journal_or_database_opens_nothing_and_changes_nothing() {
    let (run, c, result) = run(None);
    result.unwrap();
    drop(c);
    let tree = snapshot(&run.fx.root);
    let journal = fs::read(&run.tasks).unwrap();
    let mut future = journal.clone();
    future.extend_from_slice(b"{\"format\":3,\"sequence\":99}\n");
    fs::write(&run.tasks, &future).unwrap();
    let error = open_retrying(&run.fx.root, &run.fx.paths).err().unwrap();
    assert!(
        matches!(error, EngineError::NewerFormat { found: 3, .. }),
        "{error}"
    );
    assert_eq!(fs::read(&run.tasks).unwrap(), future);
    fs::write(&run.tasks, &journal).unwrap();
    // A database from a newer Studio.
    rusqlite::Connection::open(run.fx.paths.database())
        .unwrap()
        .pragma_update(None, "user_version", 77)
        .unwrap();
    let db = fs::read(run.fx.paths.database()).unwrap();
    let error = open_retrying(&run.fx.root, &run.fx.paths).err().unwrap();
    assert!(
        matches!(error, EngineError::NewerFormat { found: 77, .. }),
        "{error}"
    );
    assert_eq!(fs::read(run.fx.paths.database()).unwrap(), db);
    assert_eq!(snapshot(&run.fx.root), tree);
    // The read-only diagnostic route still works and changes nothing.
    let store = Store::open_read_only(&run.fx.paths.database()).unwrap();
    assert_eq!(store.version().unwrap(), 77);
    drop(store);
    let diagnosis = diagnose_history(&run.fx.root, &run.fx.paths).unwrap();
    assert!(diagnosis.database_newer);
    assert_eq!(diagnosis.database_version, Some(77));
    assert!(matches!(
        diagnosis.task_journal,
        TaskJournalDiagnostics::Readable { committed: 1, .. }
    ));
    fs::write(&run.tasks, &future).unwrap();
    let diagnosis = diagnose_history(&run.fx.root, &run.fx.paths).unwrap();
    assert!(matches!(
        diagnosis.task_journal,
        TaskJournalDiagnostics::Newer { found: 3 }
    ));
    assert_eq!(fs::read(&run.tasks).unwrap(), future);
    assert_eq!(fs::read(run.fx.paths.database()).unwrap(), db);
    assert_eq!(snapshot(&run.fx.root), tree);
}

#[test]
fn an_existing_version_one_installation_migrates_with_a_backup_and_keeps_its_history() {
    let fx = fixture();
    let accepted = {
        let c = open_retrying(&fx.root, &fx.paths).unwrap();
        c.state().accepted().clone()
    };
    // Make the database look like it was written before task transactions existed.
    let db = fx.paths.database();
    {
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch("DROP TABLE task_revisions; PRAGMA user_version=1;")
            .unwrap();
    }
    let before = fs::read_dir(&fx.paths.data).unwrap().count();
    let c = open_retrying(&fx.root, &fx.paths).unwrap();
    assert_eq!(c.state().accepted(), &accepted);
    assert_eq!(c.review_policy(), studio_engine::ReviewPolicy::AutoApply);
    let backups: Vec<_> = fs::read_dir(&fx.paths.data)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("backup-"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(fs::read_dir(&fx.paths.data).unwrap().count(), before + 1);
    // The lifecycle journal keeps its version-1 records; the task journal uses format 2.
    let history = fx.history(&c);
    let lifecycle = fs::read_to_string(history.join("lifecycle.jsonl")).unwrap();
    assert!(lifecycle.lines().all(|l| l.starts_with("{\"version\":1,")));
    assert_eq!(
        Store::open(&db).unwrap().version().unwrap(),
        studio_engine::store::SCHEMA_VERSION
    );
}

// ---- real process death ----------------------------------------------------------------

/// Child: applies a candidate and parks forever at the boundary named by `KILL_AT`.
#[test]
#[ignore = "spawned by real_process_death_at_publication_boundaries"]
fn kill_child() {
    let root = PathBuf::from(std::env::var("KILL_ROOT").unwrap());
    let data = PathBuf::from(std::env::var("KILL_DATA").unwrap());
    let at = std::env::var("KILL_AT").unwrap();
    let marker = data.join("boundary");
    let paths = AppPaths::new(&data).unwrap();
    let mut c = Controller::open(&root, &paths).unwrap();
    let (_, captured, report) = validated_in(&mut c, "child edit", multi_file_edit);
    c.set_transaction_hooks(Arc::new(move |b: &Boundary| {
        if format!("{b:?}") == at {
            fs::write(&marker, b"here").unwrap();
            std::thread::sleep(Duration::from_secs(120));
        }
        Ok(())
    }));
    let _ = c.complete_validated_task(&captured, &report);
}

#[test]
fn real_process_death_at_publication_boundaries() {
    for (at, committed) in [
        ("AfterDisplace(3)", false),
        ("AfterPublish(2)", false),
        ("FinalInventory", false),
        (
            "Append { event: \"commit\", op: None, step: None, after: true }",
            true,
        ),
        ("Database { after: false }", true),
    ] {
        let fx = fixture();
        let before = snapshot(&fx.root);
        let marker = fx.paths.data.join("boundary");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "kill_child", "--ignored", "--nocapture"])
            .env("KILL_ROOT", &fx.root)
            .env("KILL_DATA", &fx.paths.data)
            .env("KILL_AT", at)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !marker.exists() {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not reach {at}");
        }
        // SIGKILL: no destructor, no rollback, no flush beyond what was synced.
        child.kill().unwrap();
        child.wait().unwrap();
        let c = open_retrying(&fx.root, &fx.paths).unwrap();
        let after = snapshot(&fx.root);
        assert!(internal_files(&fx.root).is_empty(), "{at}");
        assert_eq!(
            outside_task(&before, &after, TOUCHED),
            Vec::<String>::new(),
            "{at}"
        );
        if committed {
            assert_eq!(c.state().task_history().entries().len(), 1, "{at}");
            assert_eq!(
                fs::read_to_string(fx.root.join("notes.txt")).unwrap(),
                "edited by the agent\n"
            );
        } else {
            assert!(c.state().task_history().entries().is_empty(), "{at}");
            assert_eq!(without_internal(&after), without_internal(&before), "{at}");
        }
        let _ = Path::new("");
        let _ = NoHooks;
    }
}
