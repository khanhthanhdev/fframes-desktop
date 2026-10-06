//! Studio preset mutations: source-fenced, durable, no-clobber Apply / Reapply / Reset and
//! override edits, their distinct journal record, and a process death or I/O failure at
//! every durable boundary.
#![cfg(target_os = "linux")]

#[path = "support/tx_fixture.rs"]
mod tx_fixture;

use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};
use studio_engine::{
    ApplyGate, Boundary, Controller, EngineError, PresetAction, PresetMutation, PresetRequest,
    PromotionError, TransactionKind,
    app_paths::AppPaths,
    build_materialization::sdk_pin,
    edit_transaction::TaskEvent,
    journal::TaskJournal,
    preset_state::{import_preset, installed_presets},
};
use studio_presets::builtin;
use studio_sdk::CompatibilityManifest;
use tx_fixture::*;

/// A fresh generated project (no Git: the boundary sweeps below create many of them).
fn light() -> Fx {
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
    Fx { temp, root, paths }
}

fn pkg(id: &str) -> &'static studio_presets::Package {
    builtin::get(id).unwrap_or_else(|| panic!("bundled preset {id}"))
}

fn select(
    c: &mut Controller,
    id: &str,
    action: PresetAction,
) -> Result<studio_engine::PresetOutcome, EngineError> {
    let source = c.state().source().clone();
    c.apply_preset(
        &source,
        PresetRequest::Select {
            package: pkg(id),
            action,
        },
    )
}

fn read(root: &Path, path: &str) -> Vec<u8> {
    fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn manifest_preset(root: &Path) -> Option<(String, String)> {
    let manifest = studio_project::open(root).unwrap().manifest;
    manifest.preset.map(|p| (p.id, p.sha256))
}

const OVERRIDES: &str = r##"{
  "schema": 1,
  "tokens": {
    "color.accent":   { "type": "color", "value": "#FF00AA" },
    "color.brand": { "type": "color", "value": "#112233" }
  }
}
"##;

fn journal(fx: &Fx, c: &Controller) -> studio_engine::journal::TaskReplay {
    TaskJournal::inspect(&fx.task_journal(c)).unwrap()
}

#[test]
fn apply_publishes_a_complete_snapshot_as_a_distinct_preset_revision() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    let saved = c.state().accepted().clone();
    let generation = c.state().generation();
    let before_source = c.state().source().clone();
    assert!(c.preset_mutation().is_enabled());
    assert!(c.project_style().is_none());

    let editorial = pkg("editorial");
    let outcome = select(&mut c, "editorial", PresetAction::Apply).unwrap();
    let record = outcome.record.expect("a new snapshot was committed");

    // Portable snapshot files, deterministic and covered by the source revision.
    for path in [
        "style/tokens.json",
        "style/overrides.json",
        "style/guide.md",
        "style/preset.json",
        "style/preset-tokens.json",
        "media/preset-fonts-DMSans-Medium.ttf",
        "style/preset/LICENSE.txt",
    ] {
        assert!(fx.root.join(path).is_file(), "{path}");
    }
    assert_eq!(
        manifest_preset(&fx.root),
        Some(("editorial".into(), editorial.hash().into()))
    );
    assert!(internal_files(&fx.root).is_empty());

    // Provenance is a distinct record: no task, no history, checkpoint untouched.
    assert_eq!(record.kind, TransactionKind::Preset);
    let provenance = record.preset.as_ref().unwrap();
    assert_eq!(provenance.action, PresetAction::Apply);
    assert_eq!(provenance.package_sha256, editorial.hash());
    assert_eq!(provenance.previous_preset, None);
    assert!(record.validation_report_sha256.is_empty());
    assert!(record.build.is_none());
    assert!(c.state().task_history().entries().is_empty());
    assert_eq!(c.state().accepted(), &saved);
    assert_ne!(c.state().source(), &before_source);
    assert_eq!(c.state().source(), &outcome.source);
    // The manifest reference is replaced after the file set, as one more revision.
    assert_ne!(c.state().source(), &record.published);
    assert!(c.state().promotion().is_none());

    // Derived generations are invalidated by the committed source revision.
    assert!(c.state().generation() > generation);
    assert!(c.state().built().is_none());

    let style = c.project_style().unwrap();
    assert_eq!(style.identity.id, "editorial");
    assert_eq!(style.identity.hash, editorial.hash());
    assert_eq!(
        style.tokens_sha256.as_deref(),
        Some(style.identity.tokens_hash.as_str())
    );

    // The task journal holds the transaction, but replay keeps it out of task history.
    let replay = journal(&fx, &c);
    assert!(replay.committed().is_empty());
    assert_eq!(replay.committed_presets().len(), 1);
    assert_eq!(replay.committed_presets()[0].id, record.id);
    assert!(c.undo_status_is_unavailable());
    drop(c);

    // Reopening reconstructs the same state with nothing in task history.
    let c = open_retrying(&fx.root, &fx.paths).unwrap();
    assert!(c.state().task_history().entries().is_empty());
    assert_eq!(c.project_style().unwrap().identity.id, "editorial");
}

trait UndoProbe {
    fn undo_status_is_unavailable(&mut self) -> bool;
}
impl UndoProbe for Controller {
    fn undo_status_is_unavailable(&mut self) -> bool {
        matches!(
            self.undo_status(),
            studio_engine::UndoStatus::Unavailable { .. }
        )
    }
}

#[test]
fn applying_the_snapshot_that_is_already_there_writes_nothing() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    select(&mut c, "pulse", PresetAction::Apply)
        .unwrap()
        .record
        .unwrap();
    let journal_before = fs::read(fx.task_journal(&c)).unwrap();
    let generation = c.state().generation();
    let outcome = select(&mut c, "pulse", PresetAction::Reapply).unwrap();
    assert!(outcome.record.is_none());
    assert_eq!(fs::read(fx.task_journal(&c)).unwrap(), journal_before);
    assert_eq!(c.state().generation(), generation);
}

#[test]
fn reapply_and_switch_preserve_project_overrides_byte_for_byte_and_surface_orphans() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    select(&mut c, "editorial", PresetAction::Apply).unwrap();
    // The user (or an editor) authored overrides in their own formatting.
    fs::write(fx.root.join("style/overrides.json"), OVERRIDES).unwrap();
    c.reconcile().unwrap();

    // Switching presets merges the new defaults with the very same override bytes.
    let outcome = select(&mut c, "pulse", PresetAction::Apply).unwrap();
    let record = outcome.record.unwrap();
    assert_eq!(read(&fx.root, "style/overrides.json"), OVERRIDES.as_bytes());
    assert_eq!(
        record.preset.as_ref().unwrap().previous_preset.as_deref(),
        Some("editorial")
    );
    let tokens = String::from_utf8(read(&fx.root, "style/tokens.json")).unwrap();
    assert!(tokens.to_ascii_lowercase().contains("#ff00aa"), "{tokens}");
    // The orphaned override is kept in the file and reported, never applied or dropped.
    assert!(
        outcome.notes.iter().any(|n| n.contains("color.brand")),
        "{:?}",
        outcome.notes
    );
    assert!(!tokens.contains("color.brand"));
    let style = c.project_style().unwrap();
    assert_eq!(style.identity.id, "pulse");
    assert!(
        style
            .overridden
            .iter()
            .any(|(n, layer, _)| n == "color.accent" && layer == "project")
    );
    assert!(style.diagnostics.iter().any(|d| d.contains("color.brand")));
    // The preset's own resources were replaced as a set.
    assert!(
        fx.root
            .join("media/preset-fonts-DMSans-Medium.ttf")
            .is_file()
    );

    // Reapply of the same preset keeps them too and is a no-op now.
    let again = select(&mut c, "pulse", PresetAction::Reapply).unwrap();
    assert!(again.record.is_none());
    assert_eq!(read(&fx.root, "style/overrides.json"), OVERRIDES.as_bytes());

    // Switching to a preset with different fonts removes the old preset's resources.
    select(&mut c, "quiet-motion", PresetAction::Apply).unwrap();
    assert_eq!(read(&fx.root, "style/overrides.json"), OVERRIDES.as_bytes());
    assert!(
        !fx.root
            .join("media/preset-fonts-DMSans-Medium.ttf")
            .exists()
    );
    assert!(
        fx.root
            .join("media/preset-fonts-DMSans-Regular.ttf")
            .is_file()
    );
    drop(c);

    // Overrides survive close/reopen and a later Reapply.
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    assert!(
        select(&mut c, "quiet-motion", PresetAction::Reapply)
            .unwrap()
            .record
            .is_none()
    );
    assert_eq!(read(&fx.root, "style/overrides.json"), OVERRIDES.as_bytes());
}

#[test]
fn preset_media_is_flat_and_user_media_is_never_overwritten_or_deleted() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    // A user file that a preset media file would overwrite is a refusal, not a clobber.
    fs::write(
        fx.root.join("media/preset-fonts-DMSans-Medium.ttf"),
        b"mine",
    )
    .unwrap();
    c.reconcile().unwrap();
    let error = select(&mut c, "editorial", PresetAction::Apply).unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    assert_eq!(
        read(&fx.root, "media/preset-fonts-DMSans-Medium.ttf"),
        b"mine"
    );
    assert!(!fx.root.join("style/preset.json").exists());
    fs::remove_file(fx.root.join("media/preset-fonts-DMSans-Medium.ttf")).unwrap();
    // Other user media with a similar name survives applying and switching.
    fs::write(fx.root.join("media/preset-mine.png"), b"mine").unwrap();
    fs::write(fx.root.join("media/clip.wav"), b"clip").unwrap();
    c.reconcile().unwrap();
    select(&mut c, "editorial", PresetAction::Apply).unwrap();
    // The renderer reads only top-level `media/` files: the font is one of them.
    assert!(
        fx.root
            .join("media/preset-fonts-DMSans-Medium.ttf")
            .is_file()
    );
    assert!(!fx.root.join("media/preset").exists());
    select(&mut c, "quiet-motion", PresetAction::Apply).unwrap();
    assert!(
        !fx.root
            .join("media/preset-fonts-DMSans-Medium.ttf")
            .exists()
    );
    assert!(
        fx.root
            .join("media/preset-fonts-DMSans-Regular.ttf")
            .is_file()
    );
    assert_eq!(read(&fx.root, "media/preset-mine.png"), b"mine");
    assert_eq!(read(&fx.root, "media/clip.wav"), b"clip");
}

#[test]
fn unreadable_overrides_are_kept_untouched_and_reported() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    select(&mut c, "editorial", PresetAction::Apply).unwrap();
    fs::write(fx.root.join("style/overrides.json"), "not json at all").unwrap();
    c.reconcile().unwrap();
    let outcome = select(&mut c, "pulse", PresetAction::Apply).unwrap();
    assert_eq!(read(&fx.root, "style/overrides.json"), b"not json at all");
    assert!(
        outcome.notes.iter().any(|n| n.contains("kept untouched")),
        "{:?}",
        outcome.notes
    );
}

#[test]
fn reset_is_the_only_action_that_clears_overrides() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    select(&mut c, "editorial", PresetAction::Apply).unwrap();
    fs::write(fx.root.join("style/overrides.json"), OVERRIDES).unwrap();
    c.reconcile().unwrap();
    select(&mut c, "editorial", PresetAction::Reapply).unwrap();
    assert_eq!(read(&fx.root, "style/overrides.json"), OVERRIDES.as_bytes());

    let outcome = select(&mut c, "editorial", PresetAction::Reset).unwrap();
    let record = outcome.record.unwrap();
    assert_eq!(record.preset.as_ref().unwrap().action, PresetAction::Reset);
    let overrides = String::from_utf8(read(&fx.root, "style/overrides.json")).unwrap();
    assert!(!overrides.contains("color.brand") && !overrides.contains("FF00AA"));
    let tokens = read(&fx.root, "style/tokens.json");
    assert_eq!(
        studio_presets::reresolve_project_style(
            &read(&fx.root, "style/preset.json"),
            &read(&fx.root, "style/preset-tokens.json"),
            overrides.as_bytes(),
            None,
        )
        .unwrap()
        .1
        .runtime_tokens_json()
        .into_bytes(),
        tokens
    );
    assert!(c.project_style().unwrap().overridden.is_empty());
    // The reset is the agent-free preset record; task history is still empty.
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn override_edits_set_and_clear_one_token_and_reject_what_would_not_apply() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    // No preset yet.
    let source = c.state().source().clone();
    let error = c
        .apply_preset(
            &source,
            PresetRequest::SetOverride {
                name: "color.accent",
                value: &serde_json::json!("#ff0000"),
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::Preset(studio_engine::PresetStateError::NoPreset)
    ));
    select(&mut c, "editorial", PresetAction::Apply).unwrap();

    let source = c.state().source().clone();
    let set = c
        .apply_preset(
            &source,
            PresetRequest::SetOverride {
                name: "color.accent",
                value: &serde_json::json!("#ff0000"),
            },
        )
        .unwrap();
    assert_eq!(
        set.record.unwrap().preset.unwrap().action,
        PresetAction::SetOverride
    );
    let style = c.project_style().unwrap();
    assert_eq!(style.overridden.len(), 1);
    assert_eq!(style.overridden[0].0, "color.accent");
    let tokens = String::from_utf8(read(&fx.root, "style/tokens.json")).unwrap();
    assert!(tokens.to_ascii_lowercase().contains("#ff0000"), "{tokens}");
    assert_eq!(
        style.tokens_sha256.as_deref(),
        Some(style.identity.tokens_hash.as_str())
    );

    // A token the preset does not define cannot be set (and nothing is written).
    let source = c.state().source().clone();
    let before = fs::read(fx.task_journal(&c)).unwrap();
    let error = c
        .apply_preset(
            &source,
            PresetRequest::SetOverride {
                name: "color.brand",
                value: &serde_json::json!("#112233"),
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::Preset(studio_engine::PresetStateError::NotApplied(_))
    ));
    assert_eq!(fs::read(fx.task_journal(&c)).unwrap(), before);

    // Clearing removes only that entry; clearing an unknown one is refused.
    let cleared = c
        .apply_preset(
            &source,
            PresetRequest::ClearOverride {
                name: "color.accent",
            },
        )
        .unwrap();
    assert_eq!(
        cleared.record.unwrap().preset.unwrap().action,
        PresetAction::ClearOverride
    );
    assert!(c.project_style().unwrap().overridden.is_empty());
    let source = c.state().source().clone();
    assert!(matches!(
        c.apply_preset(
            &source,
            PresetRequest::ClearOverride {
                name: "color.accent"
            }
        )
        .unwrap_err(),
        EngineError::Preset(studio_engine::PresetStateError::NotAnOverride(_))
    ));
}

#[test]
fn a_stale_source_fence_refuses_before_anything_is_written() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    let stale = c.state().source().clone();
    fs::write(fx.root.join("notes.txt"), "an editor saved this\n").unwrap();
    let before = snapshot(&fx.root);
    let error = c
        .apply_preset(
            &stale,
            PresetRequest::Select {
                package: pkg("editorial"),
                action: PresetAction::Apply,
            },
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            EngineError::Promotion(PromotionError::SourceChanged { .. })
        ),
        "{error}"
    );
    assert_eq!(snapshot(&fx.root), before);
    assert!(journal(&fx, &c).transactions.is_empty());
    // With the fresh fence the same request succeeds and keeps the editor's file.
    select(&mut c, "editorial", PresetAction::Apply).unwrap();
    assert_eq!(read(&fx.root, "notes.txt"), b"an editor saved this\n");
}

#[test]
fn an_outside_writer_during_publication_halts_as_a_conflict_that_retains_every_variant() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    let root = fx.root.clone();
    let outside = b"saved by an editor between displace and publish\n";
    c.set_transaction_hooks(
        Tape::on(
            |b| matches!(b, Boundary::AfterDisplace(_)),
            move || fs::write(root.join("style/tokens.json"), outside).unwrap(),
        )
        .hooks(),
    );
    let source = c.state().source().clone();
    let error = c
        .apply_preset(
            &source,
            PresetRequest::Select {
                package: pkg("editorial"),
                action: PresetAction::Apply,
            },
        )
        .unwrap_err();
    assert!(
        matches!(error, EngineError::Promotion(PromotionError::Conflict(_))),
        "{error}"
    );
    // The outside writer's bytes are never overwritten, and the conflict blocks more.
    assert_eq!(read(&fx.root, "style/tokens.json"), outside);
    assert!(!c.recovery_status().unresolved.is_empty());
    assert!(!c.preset_mutation().is_enabled());
    let again = c.apply_preset(
        &source,
        PresetRequest::Select {
            package: pkg("pulse"),
            action: PresetAction::Apply,
        },
    );
    assert!(matches!(
        again.unwrap_err(),
        EngineError::Promotion(PromotionError::Unresolved(_))
    ));
    assert!(c.state().task_history().entries().is_empty());
}

#[test]
fn mutation_is_disabled_when_publication_is_not_qualified_and_nothing_is_written() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    c.override_apply_gate(Some(ApplyGate::Blocked("probe failed".into())));
    let status = c.preset_mutation();
    assert!(matches!(&status, PresetMutation::Disabled(why) if why.contains("probe failed")));
    let before = snapshot(&fx.root);
    let error = select(&mut c, "editorial", PresetAction::Apply).unwrap_err();
    assert!(
        matches!(
            error,
            EngineError::Promotion(PromotionError::GateBlocked(_))
        ),
        "{error}"
    );
    assert_eq!(snapshot(&fx.root), before);
    assert!(journal(&fx, &c).transactions.is_empty());
    // The platform whose gates were measured is the only one enabled.
    const { assert!(studio_engine::preset_state::MUTATION_QUALIFIED) };
    c.override_apply_gate(None);
    assert!(c.preset_mutation().is_enabled());
}

#[test]
fn a_running_agent_task_disables_preset_changes() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    let context = start(&mut c, "edit something");
    assert!(
        matches!(c.preset_mutation(), PresetMutation::Disabled(why) if why.contains("agent task"))
    );
    assert!(select(&mut c, "editorial", PresetAction::Apply).is_err());
    drop(context);
}

#[test]
fn imported_presets_are_verified_stored_once_and_listed() {
    let fx = light();
    let export = fx.temp.path().join("export");
    studio_presets::export_dir(pkg("pulse"), &export).unwrap();
    let (none, _) = installed_presets(&fx.paths);
    assert!(none.is_empty());
    let imported = import_preset(&fx.paths, &export).unwrap();
    assert_eq!(imported.hash(), pkg("pulse").hash());
    // Importing again is idempotent.
    import_preset(&fx.paths, &export).unwrap();
    let (stored, skipped) = installed_presets(&fx.paths);
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].hash(), pkg("pulse").hash());
    // A directory that does not verify is never stored.
    let broken = fx.temp.path().join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join("preset.json"), "{}").unwrap();
    assert!(import_preset(&fx.paths, &broken).is_err());
    assert_eq!(installed_presets(&fx.paths).0.len(), 1);
}

// ---- durable boundaries ----------------------------------------------------------------

struct Run {
    fx: Fx,
    before: BTreeMap<String, Node>,
    saved: studio_project::SourceRevision,
    tasks: std::path::PathBuf,
}

/// A project that already holds editorial with user overrides, then switches to pulse
/// under `hooks`, so every kind of operation (create, replace, delete) is exercised.
fn run(hooks: Option<Tape>) -> (Run, Controller, Result<(), EngineError>) {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    select(&mut c, "editorial", PresetAction::Apply).unwrap();
    fs::write(fx.root.join("style/overrides.json"), OVERRIDES).unwrap();
    c.reconcile().unwrap();
    let saved = c.state().accepted().clone();
    let before = snapshot(&fx.root);
    if let Some(hooks) = hooks {
        c.set_transaction_hooks(hooks.hooks());
    }
    let result = select(&mut c, "quiet-motion", PresetAction::Apply).map(|o| {
        assert!(o.record.is_some());
    });
    let tasks = fx.task_journal(&c);
    (
        Run {
            fx,
            before,
            saved,
            tasks,
        },
        c,
        result,
    )
}

fn assert_recovered(run: &Run, label: &str) -> Controller {
    // The first transaction (editorial) is always committed; the switch is the second.
    let committed = journal_commits(&run.tasks) == 2;
    let c = open_retrying(&run.fx.root, &run.fx.paths)
        .unwrap_or_else(|e| panic!("{label}: reopen failed: {e}"));
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
    assert!(c.state().task_history().entries().is_empty(), "{label}");
    let after = snapshot(&run.fx.root);
    let overrides = read(&run.fx.root, "style/overrides.json");
    assert_eq!(
        overrides,
        OVERRIDES.as_bytes(),
        "{label}: overrides changed"
    );
    if committed {
        // The complete new file set, manifest included.
        let quiet = pkg("quiet-motion");
        assert_eq!(
            manifest_preset(&run.fx.root),
            Some(("quiet-motion".into(), quiet.hash().into())),
            "{label}"
        );
        assert!(
            run.fx
                .root
                .join("media/preset-fonts-DMSans-Regular.ttf")
                .is_file(),
            "{label}"
        );
        assert!(
            !run.fx
                .root
                .join("media/preset-fonts-DMSans-Medium.ttf")
                .exists(),
            "{label}"
        );
        assert_eq!(
            c.project_style().unwrap().identity.id,
            "quiet-motion",
            "{label}"
        );
    } else {
        // The complete old file set, byte for byte and mode for mode.
        assert_eq!(
            without_internal(&after),
            without_internal(&run.before),
            "{label}"
        );
        assert_eq!(
            c.project_style().unwrap().identity.id,
            "editorial",
            "{label}"
        );
        assert_eq!(
            manifest_preset(&run.fx.root).unwrap().0,
            "editorial",
            "{label}"
        );
    }
    // Replay never leaks a preset record into task history.
    let replay = TaskJournal::inspect(&run.tasks).unwrap();
    assert!(replay.committed().is_empty(), "{label}");
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
fn the_preset_boundary_sequence_covers_every_durable_step() {
    let tape = Tape::recording();
    let (_, _c, result) = run(Some(tape.clone()));
    result.unwrap();
    let seen = tape.boundaries();
    let has = |f: &dyn Fn(&Boundary) -> bool| seen.iter().any(f);
    assert!(has(&|b| matches!(b, Boundary::StageCreated(_))));
    assert!(has(&|b| matches!(b, Boundary::StageSynced(_))));
    assert!(has(&|b| matches!(b, Boundary::AfterDisplace(_))));
    assert!(has(&|b| matches!(b, Boundary::AfterPublish(_))));
    assert!(has(&|b| matches!(b, Boundary::PublishVerified(_))));
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
    assert!(has(&|b| *b == Boundary::Manifest { after: false }));
    assert!(has(&|b| *b == Boundary::Manifest { after: true }));
    assert!(has(&|b| matches!(b, Boundary::Cleanup(_))));
    // The preset path never touches the task history projection.
    assert!(!has(&|b| matches!(b, Boundary::Projection { .. })));
}

#[test]
fn a_process_death_at_every_durable_boundary_recovers_to_the_old_or_new_complete_set() {
    let total = boundary_count();
    assert!(total > 40, "unexpectedly few boundaries: {total}");
    for k in 0..total {
        let (run, c, result) = run(Some(Tape::crashing_at(k)));
        let error = result.unwrap_err();
        assert!(is_crash(&error), "boundary {k}: {error}");
        assert!(
            c.state().task_history().entries().is_empty(),
            "boundary {k}: preset leaked into task history"
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
        match &result {
            Ok(()) => assert_eq!(journal_commits(&run.tasks), 2, "{label}"),
            Err(EngineError::Promotion(
                PromotionError::RolledBack(_) | PromotionError::Journal(_),
            )) => (),
            Err(other) => panic!("{label}: {other}"),
        }
        assert!(c.state().task_history().entries().is_empty(), "{label}");
        drop(c);
        drop(assert_recovered(&run, &label));
        assert_idempotent(&run, &label);
    }
}

#[test]
fn a_journal_that_gains_a_preset_record_stays_readable_with_unchanged_task_events() {
    let fx = light();
    let mut c = open_retrying(&fx.root, &fx.paths).unwrap();
    select(&mut c, "pulse", PresetAction::Apply).unwrap();
    let text = fs::read_to_string(fx.task_journal(&c)).unwrap();
    // Events keep their format and labels; only the record gains `kind: preset`.
    let intent = text.lines().next().unwrap();
    assert!(intent.contains("\"Intent\""));
    assert!(intent.contains("\"kind\":\"preset\""));
    let replay = journal(&fx, &c);
    let tx = &replay.transactions[0];
    assert!(matches!(
        tx.status,
        studio_engine::journal::TxStatus::Committed(_)
    ));
    assert_eq!(
        TaskEvent::Resolved {
            note: String::new()
        }
        .label(),
        "resolved"
    );
}
