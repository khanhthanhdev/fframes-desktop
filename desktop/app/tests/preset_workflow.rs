//! The native preset controls against the REAL engine controller, the real preview
//! coordinator and the deterministic fake preview worker: selection, the CSS report,
//! override / reapply / reset, source conflicts, a failed preview and old-preview retention.
//! No window is opened; the panel's model and the shell's handoff rules are pure functions
//! of the reports and snapshots proven here (the GPUI process is exercised by `x11_shell`).
#![cfg(target_os = "linux")]

use fframes_studio::{
    build_service::{BuildLimits, BuildService},
    preset_panel::{Catalog, Outcome, PresetCommand, PresetReport, PresetView, Refusal, execute},
    preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent},
};
use parking_lot::Mutex;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    ApplyGate, Boundary, Controller, JobKind, JobResult, PresetMutation, PreviewState,
    app_paths::AppPaths,
};
use studio_project::SourceRevision;

#[path = "support/build_fixture.rs"]
mod fixture;
use fixture::*;

struct World {
    temp: tempfile::TempDir,
    sdk: PathBuf,
    root: PathBuf,
    paths: AppPaths,
    controller: Arc<Mutex<Controller>>,
    compiler: Arc<FakeCompiler>,
    service: BuildService,
    catalog: Catalog,
}

fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let sdk = fake_sdk(temp.path());
    let root = temp.path().join("video");
    create_project(&root);
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    let controller = Controller::open(&root, &paths).unwrap();
    let compiler = FakeCompiler::new(true);
    let service = BuildService::new(
        ProcessTreeManager::new(),
        compiler.clone(),
        BuildLimits::default(),
    );
    let catalog = Catalog::load(&paths);
    World {
        temp,
        sdk,
        root,
        paths,
        controller: Arc::new(Mutex::new(controller)),
        compiler,
        service,
        catalog,
    }
}

impl World {
    /// The revision a fresh window would show (the UI fence).
    fn shown(&self) -> SourceRevision {
        self.controller.lock().state().source().clone()
    }

    fn run(&mut self, command: PresetCommand) -> PresetReport {
        let shown = self.shown();
        self.run_at(command, &shown)
    }

    fn run_at(&mut self, command: PresetCommand, shown: &SourceRevision) -> PresetReport {
        execute(
            command,
            &self.controller,
            &mut self.catalog,
            &self.paths,
            Some(shown),
        )
    }

    fn view(&mut self) -> PresetView {
        PresetView::capture(&mut self.controller.lock(), &self.catalog)
    }

    fn key(&mut self, id: &str) -> String {
        self.view()
            .entries
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("catalog has {id}"))
            .key
            .clone()
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.root.join(path)).unwrap()
    }

    /// Builds the current source through the coordinator exactly as the shell's Build
    /// command does and waits for the outcome.
    fn build(
        &self,
        coordinator: &PreviewCoordinator,
        state: &mut PreviewState,
    ) -> Result<Arc<studio_engine::ReadyPreview>, String> {
        let mut c = self.controller.lock();
        let tag = c.begin_job(JobKind::Build).unwrap();
        state.begin(tag.clone());
        coordinator.build(BuildSpec {
            project: c.project.clone(),
            sdk: self.sdk.clone(),
            compatibility: manifest(),
            builds: self.paths.builds(),
            tag,
            compiler: c.operation_processes(),
            worker: c.processes.sub_manager(),
            service: self.service.clone(),
        });
        drop(c);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let e = coordinator.events();
            if let Some(tag) = e.compiled {
                self.controller
                    .lock()
                    .complete(&tag, JobResult::Built(tag.base_source.clone()))
                    .unwrap();
            }
            if let Some((tag, error)) = e.error {
                let mut c = self.controller.lock();
                if c.state().active_tag() == Some(&tag) {
                    let _ = c.complete(&tag, JobResult::Failed(error.clone()));
                }
                state.fail(&tag, error.clone());
                return Err(error);
            }
            if let Some(ready) = e.ready {
                self.controller.lock().reconcile().unwrap();
                return Ok(ready);
            }
            assert!(Instant::now() < deadline, "preview deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Displays the current source so "old playback" exists.
    fn display(&self) -> (PreviewCoordinator, PreviewState) {
        let coordinator = PreviewCoordinator::new(self.controller.lock().processes.sub_manager());
        let mut state = PreviewState::default();
        let ready = self.build(&coordinator, &mut state).expect("base preview");
        assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
        state
            .install(&ready, self.controller.lock().state())
            .unwrap();
        (coordinator, state)
    }

    fn install(
        &self,
        coordinator: &PreviewCoordinator,
        state: &mut PreviewState,
        ready: &Arc<studio_engine::ReadyPreview>,
    ) {
        assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
        state
            .install(ready, self.controller.lock().state())
            .unwrap();
    }
}

fn seek_and_wait(p: &PreviewCoordinator, state: &mut PreviewState, position: usize) {
    let serial = state.seek(position, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: state.displayed().unwrap().clone(),
        serial,
        position,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(f) = p.events().frame
            && state.accepts_frame(&f)
        {
            assert_eq!(f.response.frame_index, position);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the displayed worker stopped answering"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn committed(report: &PresetReport) -> (&str, &[String]) {
    match &report.outcome {
        Outcome::Committed { source, notes, .. } => (source, notes),
        other => panic!("expected a committed revision, got {other:?}"),
    }
}

/// A preset directory with another identity (a distinct, verified package).
fn custom_preset_dir(root: &Path) -> PathBuf {
    let dir = root.join("custom-preset");
    studio_presets::export_dir(studio_presets::builtin::get("pulse").unwrap(), &dir).unwrap();
    let manifest = fs::read_to_string(dir.join("preset.json")).unwrap();
    let manifest = manifest
        .replace("\"id\": \"pulse\"", "\"id\": \"night-owl\"")
        .replace("\"name\": \"Pulse\"", "\"name\": \"Night Owl\"");
    fs::write(dir.join("preset.json"), manifest).unwrap();
    dir
}

#[test]
fn selection_lists_bundled_and_imported_presets_and_applies_the_chosen_one() {
    let mut w = world();
    let view = w.view();
    assert_eq!(
        view.entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        ["editorial", "pulse", "quiet-motion"]
    );
    assert!(view.entries.iter().all(|e| e.bundled && !e.applied));
    assert!(view.applied.is_none());
    assert_eq!(view.mutation, PresetMutation::Enabled);

    let key = w.key("editorial");
    let report = w.run(PresetCommand::Apply { key });
    let (source, _) = committed(&report);
    assert_eq!(source, &w.shown().as_str()[..12]);
    let view = w.view();
    assert_eq!(view.applied.as_ref().unwrap().id, "editorial");
    assert!(
        view.entries
            .iter()
            .find(|e| e.id == "editorial")
            .unwrap()
            .applied
    );

    // An imported package joins the catalog (and survives a reload of the store).
    let dir = custom_preset_dir(w.temp.path());
    let report = w.run(PresetCommand::ImportFolder(dir));
    assert!(report.catalog_changed, "{report:?}");
    assert!(matches!(&report.outcome, Outcome::Done(text) if text.contains("Night Owl")));
    let reloaded = Catalog::load(&w.paths);
    assert!(
        reloaded
            .entries(None)
            .iter()
            .any(|e| e.id == "night-owl" && !e.bundled)
    );
    let key = w.key("night-owl");
    let report = w.run(PresetCommand::Apply { key });
    committed(&report);
    assert_eq!(w.view().applied.unwrap().id, "night-owl");
    // The project carries a self-contained snapshot: nothing points into the store.
    assert!(
        w.root
            .join("media/preset-fonts-DMSans-Medium.ttf")
            .is_file()
    );
    assert!(w.root.join("style/preset/LICENSE.txt").is_file());
    // Export writes the package directory with its licenses.
    let key = w.key("night-owl");
    let dest = w.temp.path().join("exported");
    let report = w.run(PresetCommand::Export {
        key,
        dest: dest.clone(),
    });
    assert!(matches!(report.outcome, Outcome::Done(_)), "{report:?}");
    assert!(dest.join("licenses/DMSans-OFL.txt").is_file());
    // A non-preset directory is refused and never stored.
    let broken = w.temp.path().join("broken");
    fs::create_dir(&broken).unwrap();
    let report = w.run(PresetCommand::ImportFolder(broken));
    assert!(matches!(
        report.outcome,
        Outcome::Refused(Refusal::Invalid(_))
    ));
    assert!(!report.catalog_changed);
}

#[test]
fn the_css_report_lists_every_declaration_and_writes_nothing_to_the_project() {
    let mut w = world();
    let css = w.temp.path().join("brand.css");
    fs::write(
        &css,
        ":root {\n  --color-accent: #ff5533;\n  --color-background: rgb(10, 20, 30);\n  --not-a-token: 3;\n  --spacing-margin: banana;\n}\n",
    )
    .unwrap();
    let source_before = w.shown();
    let report = w.run(PresetCommand::ImportCss(css));
    let view = report.css.as_ref().expect("a report view");
    assert!(view.accepted >= 1, "{view:?}");
    assert!(view.unsupported + view.rejected >= 1, "{view:?}");
    assert_eq!(
        view.lines.len(),
        view.accepted + view.unsupported + view.rejected
    );
    assert!(view.lines.iter().any(|l| l.contains("accepted")));
    assert!(report.preview_request().is_none());
    // Reading a CSS file is not a mutation: no source change, no journal entry.
    assert_eq!(w.shown(), source_before);
    let journal = w
        .paths
        .project(&w.controller.lock().project.manifest.project_id)
        .join("tasks.jsonl");
    assert_eq!(fs::read(journal).unwrap_or_default(), b"");
    // A missing file is a refusal, not a panic.
    let missing = w.run(PresetCommand::ImportCss(w.temp.path().join("nope.css")));
    assert!(missing.refused());
}

#[test]
fn override_reapply_and_reset_keep_the_users_layer_until_it_is_explicitly_reset() {
    let mut w = world();
    let key = w.key("editorial");
    committed(&w.run(PresetCommand::Apply { key: key.clone() }));

    let set = w.run(PresetCommand::SetOverride {
        token: "color.accent".into(),
        value: "#ff0000".into(),
    });
    committed(&set);
    let view = w.view();
    let applied = view.applied.unwrap();
    assert_eq!(applied.overridden.len(), 1);
    assert_eq!(applied.overridden[0].0, "color.accent");
    let overrides = w.read("style/overrides.json");

    // Reapply keeps the override layer byte for byte and (unchanged package) is a no-op.
    let report = w.run(PresetCommand::Reapply { key: key.clone() });
    assert!(
        matches!(report.outcome, Outcome::Unchanged { .. }),
        "{report:?}"
    );
    assert!(report.preview_request().is_none());
    assert_eq!(w.read("style/overrides.json"), overrides);

    // Switching preset keeps it too.
    let pulse = w.key("pulse");
    committed(&w.run(PresetCommand::Apply { key: pulse }));
    assert_eq!(w.read("style/overrides.json"), overrides);
    assert_eq!(w.view().applied.unwrap().overridden.len(), 1);

    // A token the preset does not define is refused with a reason.
    let refused = w.run(PresetCommand::SetOverride {
        token: "color.nowhere".into(),
        value: "#000000".into(),
    });
    assert!(
        matches!(&refused.outcome, Outcome::Refused(Refusal::Invalid(why)) if why.contains("not")),
        "{refused:?}"
    );

    // Clearing removes only that entry.
    committed(&w.run(PresetCommand::ClearOverride {
        token: "color.accent".into(),
    }));
    assert!(w.view().applied.unwrap().overridden.is_empty());

    // Reset is a separate, explicit action that clears the layer.
    committed(&w.run(PresetCommand::SetOverride {
        token: "color.text".into(),
        value: "#00ff00".into(),
    }));
    let key = w.key("pulse");
    let reset = w.run(PresetCommand::Reset { key });
    committed(&reset);
    assert!(w.view().applied.unwrap().overridden.is_empty());
    let text = String::from_utf8(w.read("style/overrides.json")).unwrap();
    assert!(!text.contains("color.text"));
}

#[test]
fn a_stale_view_or_an_outside_writer_is_reported_as_a_conflict_and_requests_no_preview() {
    let mut w = world();
    let stale = w.shown();
    fs::write(w.root.join("notes.txt"), "an editor saved this\n").unwrap();
    let key = w.key("editorial");
    let before = fs::read(w.root.join("studio.json")).unwrap();
    let report = w.run_at(PresetCommand::Apply { key: key.clone() }, &stale);
    assert!(
        matches!(report.outcome, Outcome::Refused(Refusal::SourceChanged(_))),
        "{report:?}"
    );
    assert!(report.preview_request().is_none());
    assert!(report.status_lines()[0].contains("Nothing was written"));
    assert_eq!(fs::read(w.root.join("studio.json")).unwrap(), before);
    assert!(!w.root.join("style/preset.json").exists());

    // An outside writer recreating a destination during publication halts as a conflict:
    // its bytes survive, every variant is retained and no preview is requested.
    let root = w.root.clone();
    let outside = b"saved by an editor during publication\n";
    w.controller.lock().set_transaction_hooks(Arc::new({
        let fired = std::sync::atomic::AtomicBool::new(false);
        move |b: &Boundary| {
            if matches!(b, Boundary::AfterDisplace(_)) && !fired.swap(true, Ordering::SeqCst) {
                fs::write(root.join("style/tokens.json"), outside).unwrap();
            }
            Ok(())
        }
    }));
    let report = w.run(PresetCommand::Apply { key });
    let Outcome::Refused(Refusal::Conflict { variants, .. }) = &report.outcome else {
        panic!("expected a conflict, got {report:?}");
    };
    assert!(!variants.is_empty());
    assert!(report.preview_request().is_none());
    assert!(
        report
            .status_lines()
            .join(" ")
            .contains("every variant was kept")
    );
    assert_eq!(w.read("style/tokens.json"), outside);
    // The conflict blocks further mutation until resolved; the view says why.
    assert!(matches!(w.view().mutation, PresetMutation::Disabled(_)));
    let pulse = w.key("pulse");
    let report = w.run(PresetCommand::Apply { key: pulse });
    assert!(
        matches!(report.outcome, Outcome::Refused(Refusal::Blocked(_))),
        "{report:?}"
    );
}

#[test]
fn a_blocked_publication_gate_disables_the_controls_and_writes_nothing() {
    let mut w = world();
    w.controller
        .lock()
        .override_apply_gate(Some(ApplyGate::Blocked("probe failed".into())));
    assert!(
        matches!(w.view().mutation, PresetMutation::Disabled(why) if why.contains("probe failed"))
    );
    let key = w.key("editorial");
    let before = fs::read(w.root.join("studio.json")).unwrap();
    let report = w.run(PresetCommand::Apply { key });
    assert!(
        matches!(report.outcome, Outcome::Refused(Refusal::Blocked(_))),
        "{report:?}"
    );
    assert!(report.preview_request().is_none());
    assert_eq!(fs::read(w.root.join("studio.json")).unwrap(), before);
    assert!(!w.root.join("style/preset.json").exists());
}

#[test]
fn a_committed_revision_requests_a_matching_preview_that_replaces_the_old_one() {
    let mut w = world();
    let (coordinator, mut state) = w.display();
    let old = state.displayed().unwrap().clone();

    let key = w.key("editorial");
    let report = w.run(PresetCommand::Apply { key });
    committed(&report);
    // The commit invalidated the build of the old source, but the old preview still plays.
    assert!(w.controller.lock().state().built().is_none());
    assert_eq!(state.displayed(), Some(&old));
    seek_and_wait(&coordinator, &mut state, 3);
    // The shell's label for the revision still waiting on its preview.
    let request = report
        .preview_request()
        .expect("committed asks for a preview");
    assert!(
        request.label.contains("previous revision"),
        "{}",
        request.label
    );

    // The ordinary revision-safe build of the committed source replaces it.
    let ready = w.build(&coordinator, &mut state).expect("matching preview");
    let source = w.shown();
    assert_eq!(ready.identity().source_revision, source.as_str());
    assert_ne!(ready.identity().source_revision, old.source_revision);
    w.install(&coordinator, &mut state, &ready);
    assert_eq!(state.displayed().unwrap().source_revision, source.as_str());
    coordinator.close();
}

#[test]
fn a_failed_preview_after_a_committed_revision_keeps_the_previous_playable_preview() {
    let mut w = world();
    let (coordinator, mut state) = w.display();
    let old = state.displayed().unwrap().clone();

    w.compiler.fail.store(true, Ordering::SeqCst);
    let key = w.key("quiet-motion");
    let report = w.run(PresetCommand::Apply { key });
    committed(&report);
    let request = report.preview_request().unwrap();
    let committed_source = w.shown();
    assert_ne!(committed_source.as_str(), old.source_revision);

    // The preview of the committed source fails to build.
    let error = w.build(&coordinator, &mut state).unwrap_err();
    assert!(error.contains("E0425"), "{error}");
    // The previous revision is still the displayed, seekable, playable one...
    assert_eq!(state.displayed(), Some(&old));
    seek_and_wait(&coordinator, &mut state, 7);
    // ...the preset commit was not undone, and it stays labeled as awaiting its preview.
    assert_eq!(w.shown(), committed_source);
    assert_eq!(w.view().applied.unwrap().id, "quiet-motion");
    assert!(
        request.label.contains(&committed_source.as_str()[..12])
            || request.label.contains("source")
    );
    assert!(request.label.contains("still shows the previous revision"));

    // Repairing the build lets the matching preview replace the old one.
    w.compiler.fail.store(false, Ordering::SeqCst);
    let ready = w.build(&coordinator, &mut state).expect("retry");
    w.install(&coordinator, &mut state, &ready);
    assert_eq!(
        state.displayed().unwrap().source_revision,
        committed_source.as_str()
    );
    coordinator.close();
}

#[test]
fn panel_colors_never_come_from_preset_data() {
    // The preset panel is Studio chrome: no `rgb(...)` in it is computed from a snapshot,
    // token, override, CSS entry or catalog entry, and it never reads runtime token values.
    let source = include_str!("../src/preset_panel.rs");
    let production = source.split("#[cfg(test)]").next().unwrap();
    for (n, line) in production.lines().enumerate() {
        let Some(at) = line.find("rgb(") else {
            continue;
        };
        let argument = line[at + 4..]
            .split(')')
            .next()
            .unwrap()
            .to_ascii_lowercase();
        for forbidden in [
            "token", "applied", "entry", "css", "overrid", "view", "value",
        ] {
            assert!(
                !argument.contains(forbidden),
                "line {}: chrome color {argument:?} depends on preset data",
                n + 1
            );
        }
    }
    assert!(!production.contains("runtime_tokens"));
    assert!(!production.contains("TokenValue"));
    assert!(!production.contains("Color::"));
}
