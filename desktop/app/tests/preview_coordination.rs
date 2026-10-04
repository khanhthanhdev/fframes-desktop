use fframes_studio::{
    preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent},
    worker_project::acquire_build_lock,
};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    Controller, JobKind, JobResult, PreviewState, ReadyPreview, app_paths::AppPaths,
    build_materialization::sdk_pin,
};
use studio_sdk::{CompatibilityManifest, SdkInstaller};

#[test]
fn blocked_target_lock_cancel_is_bounded_and_does_not_unlock_another_build() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("lock");
    let owner = ProcessTreeManager::new();
    let held = acquire_build_lock(&path, &owner, Duration::from_secs(1)).unwrap();
    let waiting = owner.sub_manager();
    let scope = waiting.clone();
    let lock_path = path.clone();
    let start = Instant::now();
    let task =
        std::thread::spawn(move || acquire_build_lock(&lock_path, &scope, Duration::from_secs(20)));
    std::thread::sleep(Duration::from_millis(50));
    waiting.shutdown(Duration::ZERO);
    assert!(task.join().unwrap().unwrap_err().contains("cancelled"));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(acquire_build_lock(&path, &owner, Duration::from_millis(30)).is_err());
    drop(held);
    assert!(acquire_build_lock(&path, &owner, Duration::from_secs(1)).is_ok());
}

fn spec(
    c: &mut Controller,
    sdk: &std::path::Path,
    m: &CompatibilityManifest,
    paths: &AppPaths,
) -> BuildSpec {
    let tag = c.begin_job(JobKind::Build).unwrap();
    BuildSpec {
        project: c.project.clone(),
        sdk: sdk.into(),
        compatibility: m.clone(),
        builds: paths.builds(),
        tag,
        compiler: c.operation_processes(),
        worker: c.processes.sub_manager(),
    }
}
fn wait_ready(p: &PreviewCoordinator, c: &mut Controller) -> Arc<ReadyPreview> {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let e = p.events();
        if let Some(tag) = e.compiled {
            c.complete(&tag, JobResult::Built(tag.base_source.clone()))
                .unwrap();
        }
        if let Some((_, error)) = e.error {
            panic!("candidate failed: {error}");
        }
        if let Some(r) = e.ready {
            c.reconcile().unwrap();
            return r;
        }
        assert!(
            Instant::now() < deadline,
            "preview build readiness deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
#[ignore = "requires fresh M2 SDK_BUNDLE; explicitly run for real compiler/worker qualification"]
fn real_failed_build_and_invalid_source_preserve_seekable_immutable_revision_and_cleanup() {
    let bundle = PathBuf::from(std::env::var_os("SDK_BUNDLE").expect("SDK_BUNDLE"));
    let m = CompatibilityManifest::from_json_str(
        &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let artifacts: Vec<_> = m
        .artifacts
        .iter()
        .map(|a| (a.clone(), bundle.join(a.url.trim_start_matches("file://"))))
        .collect();
    let sdk = SdkInstaller::new(temp.path().join("sdk"))
        .install_from_local_artifacts(&m, &artifacts)
        .unwrap();
    let root = temp.path().join("video");
    studio_project::create(
        &root,
        "Revision test",
        sdk_pin(&m),
        &m.fframes_version,
        "0.1.0",
    )
    .unwrap();
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let mut c = Controller::open(&root, &paths).unwrap();
    let accepted = c.state().accepted().clone();
    let parent = c.processes.clone();
    let p = PreviewCoordinator::new(parent.sub_manager());
    let mut state = PreviewState::default();
    let first = spec(&mut c, &sdk, &m, &paths);
    state.begin(first.tag.clone());
    p.build(first);
    let r = wait_ready(&p, &mut c);
    assert!(!p.commit(r.identity().clone(), r.seek_serial + 1));
    assert!(p.commit(r.identity().clone(), r.seek_serial));
    state.install(&r, c.state()).unwrap();
    assert_eq!(
        (
            r.frame.as_ref().unwrap().response.header.width,
            r.frame.as_ref().unwrap().response.header.height
        ),
        (1280, 720)
    );
    assert_eq!(&r.frame.as_ref().unwrap().pixels[..4], &[13, 17, 23, 255]);
    let original = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert_eq!(c.state().accepted(), &accepted);
    let font = root.join("media/DMSans-Medium.ttf");
    let bytes = fs::read(&font).unwrap();
    fs::remove_file(&font).unwrap();
    assert!(c.reconcile().is_err());
    let serial = state.seek(17, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial,
        position: 17,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(f) = p.events().frame {
            assert!(state.accepts_frame(&f));
            assert_eq!(f.response.frame_index, 17);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "old worker must remain seekable without live font"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(&font, bytes).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "compile_error!(\"deliberate broken candidate\");",
    )
    .unwrap();
    c.reconcile().unwrap();
    let broken = spec(&mut c, &sdk, &m, &paths);
    let broken_tag = broken.tag.clone();
    state.begin(broken.tag.clone());
    p.build(broken);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let e = p.events();
        assert!(e.ready.is_none(), "broken compile cannot install a preview");
        if let Some((tag, error)) = e.error {
            assert_eq!(tag, broken_tag);
            c.complete(&tag, JobResult::Failed(error.clone())).unwrap();
            state.fail(&tag, error);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    // Send asymmetric rapid intents; a late response for 7 must not overwrite 43.
    let old = state.seek(7, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial: old,
        position: 7,
        scale: state.scale(),
    });
    let newest = state.seek(43, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial: newest,
        position: 43,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(f) = p.events().frame
            && state.accepts_frame(&f)
        {
            assert_eq!(f.response.frame_index, 43);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(state.displayed(), Some(r.identity()));
    assert_eq!(c.state().accepted(), &accepted);
    assert_eq!(
        fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "compile_error!(\"deliberate broken candidate\");"
    );
    // Scene-boundary inspection alone misses media used only at the playhead.
    let missing = original.replace(
        "        fframes::svgr!(",
        "        if _frame.index == 43 || _frame.index == 19 { let _ = _ctx.get_image(\"absent.jpg\"); }\n        fframes::svgr!(",
    );
    assert_ne!(missing, original);
    fs::write(root.join("src/lib.rs"), &missing).unwrap();
    c.reconcile().unwrap();
    let broken = spec(&mut c, &sdk, &m, &paths);
    let tag = broken.tag.clone();
    state.begin(tag.clone());
    p.build(broken);
    wait_inspection_failure(&p, &mut c, &mut state, &tag);
    assert_eq!(state.displayed(), Some(r.identity()));

    // The same error must also be caught when a ready candidate is re-primed.
    seek_and_wait(&p, &mut state, 17);
    let candidate = spec(&mut c, &sdk, &m, &paths);
    let tag = candidate.tag.clone();
    state.begin(tag.clone());
    p.build(candidate);
    let obsolete = wait_ready(&p, &mut c);
    assert_eq!(obsolete.position, 17);
    let serial = state.seek(19, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial,
        position: 19,
        scale: state.scale(),
    });
    assert!(!p.commit(obsolete.identity().clone(), obsolete.seek_serial));
    wait_inspection_failure(&p, &mut c, &mut state, &tag);
    assert_eq!(state.displayed(), Some(r.identity()));
    drop(obsolete);

    // Block only the candidate's frame 23. The displayed worker must still
    // deliver the newest seek before the candidate is allowed to finish.
    let started = temp.path().join("reprime-started");
    let release = temp.path().join("reprime-release");
    let hook = format!(
        "        if _frame.index == 23 {{\n            std::fs::write({started:?}, b\"started\").unwrap();\n            while !std::path::Path::new({release:?}).exists() {{ std::thread::sleep(std::time::Duration::from_millis(5)); }}\n        }}\n        fframes::svgr!(",
        started = started.to_str().unwrap(),
        release = release.to_str().unwrap(),
    );
    fs::write(
        root.join("src/lib.rs"),
        original.replace("        fframes::svgr!(", &hook),
    )
    .unwrap();
    c.reconcile().unwrap();
    let candidate = spec(&mut c, &sdk, &m, &paths);
    state.begin(candidate.tag.clone());
    p.build(candidate);
    let obsolete = wait_ready(&p, &mut c);
    let serial = state.seek(23, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial,
        position: 23,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() {
        assert!(
            Instant::now() < deadline,
            "candidate never started re-priming"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    seek_and_wait(&p, &mut state, 37);
    assert!(!release.exists(), "displayed seek must not await candidate");
    assert!(!p.commit(obsolete.identity().clone(), obsolete.seek_serial));
    drop(obsolete);
    fs::write(&release, b"release").unwrap();
    let latest = wait_ready(&p, &mut c);
    assert_eq!(latest.position, 37);
    assert_eq!(latest.seek_serial, state.serial());
    assert!(state.can_install(&latest, c.state()).is_ok());

    // Cancel a second blocked re-prime and prove no late candidate is published.
    fs::remove_file(&started).unwrap();
    fs::remove_file(&release).unwrap();
    let serial = state.seek(23, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial,
        position: 23,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let cancelled = Instant::now();
    p.cancel_build();
    state.cancel_build();
    assert!(cancelled.elapsed() < Duration::from_secs(2));
    assert!(!p.commit(latest.identity().clone(), latest.seek_serial));
    drop(latest);
    seek_and_wait(&p, &mut state, 43);
    assert!(p.events().ready.is_none());
    assert_eq!(state.displayed(), Some(r.identity()));
    assert_eq!(c.state().accepted(), &accepted);

    // The new shorter worker is re-primed after a seek that races its ready event.
    fs::write(
        root.join("src/lib.rs"),
        original.replace("Seconds(5.0)", "Seconds(1.0)"),
    )
    .unwrap();
    c.reconcile().unwrap();
    let shorter = spec(&mut c, &sdk, &m, &paths);
    state.begin(shorter.tag.clone());
    p.build(shorter);
    let obsolete = wait_ready(&p, &mut c);
    assert_eq!(obsolete.position, 30);
    let serial = state.seek(19, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: r.identity().clone(),
        serial,
        position: 19,
        scale: state.scale(),
    });
    assert!(!p.commit(obsolete.identity().clone(), obsolete.seek_serial));
    assert!(state.can_install(&obsolete, c.state()).is_err());
    drop(obsolete);
    let latest = wait_ready(&p, &mut c);
    assert_eq!(latest.position, 19);
    assert_eq!(latest.seek_serial, serial);
    assert_eq!(latest.pcm_start_sample, 30400);
    assert!(p.commit(latest.identity().clone(), serial));
    state.install(&latest, c.state()).unwrap();
    assert_eq!(state.displayed(), Some(latest.identity()));
    assert_eq!(state.position(), 19);
    assert_eq!(c.state().accepted(), &accepted);
    // A subsequent build cancellation must not shut down this committed worker.
    p.cancel_build();
    let serial = state.seek(29, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: latest.identity().clone(),
        serial,
        position: 29,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(f) = p.events().frame
            && state.accepts_frame(&f)
        {
            assert_eq!(f.response.frame_index, 29);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    p.close();
    drop(p);
    c.close().unwrap();
    assert_eq!(parent.active_count(), 0);
    // Ready previews intentionally retain open PCM and its materialization.
    // Release the final consumers before checking coordinator cleanup.
    drop(r);
    drop(latest);
    fn assert_no_live_trees(path: &std::path::Path) {
        for e in fs::read_dir(path).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                assert!(
                    !p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("build-"),
                    "materialization leaked: {}",
                    p.display()
                );
                // Shared target outputs are an intentional reusable compiler cache.
                if p.file_name().unwrap() != "targets" {
                    assert_no_live_trees(&p);
                }
            }
        }
    }
    assert_no_live_trees(&paths.builds());
}

fn wait_inspection_failure(
    p: &PreviewCoordinator,
    c: &mut Controller,
    state: &mut PreviewState,
    tag: &studio_engine::OperationTag,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let e = p.events();
        if let Some(compiled) = e.compiled {
            c.complete(&compiled, JobResult::Built(compiled.base_source.clone()))
                .unwrap();
        }
        assert!(
            e.ready.is_none(),
            "missing media at the install frame became ready"
        );
        if let Some((failed, error)) = e.error {
            assert_eq!(&failed, tag);
            assert!(error.contains("absent.jpg"), "{error}");
            state.fail(&failed, error);
            return;
        }
        assert!(Instant::now() < deadline, "inspection did not fail");
        std::thread::sleep(Duration::from_millis(5));
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
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(f) = p.events().frame
            && state.accepts_frame(&f)
        {
            assert_eq!(f.response.frame_index, position);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "last-good worker seek was blocked"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
