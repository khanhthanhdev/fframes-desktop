use std::sync::Arc;

use fframes_studio::{frame_image, thumbnail_cache};
use fframes_studio_protocol::PreviewIdentity;
use gpui::RenderImage;
use thumbnail_cache::{MAX_THUMBNAIL_BYTES, MAX_THUMBNAIL_ENTRIES, ThumbnailCache, ThumbnailKey};

#[path = "support/preview_fixture.rs"]
mod preview_fixture;

fn identity(revision: &str, generation: u64) -> PreviewIdentity {
    PreviewIdentity {
        project_id: "project".into(),
        open_session: "session".into(),
        source_revision: revision.into(),
        worker_generation: generation,
    }
}

fn key(frame: usize) -> ThumbnailKey {
    ThumbnailKey::new(identity("revision", 1), 0.25, frame)
}

fn image(frame: usize) -> Arc<RenderImage> {
    let (header, pixels) =
        frame_image::generate_reference_frame("revision", 1, 1, frame, 4, 4, false);
    frame_image::create_render_image(&header, &pixels).expect("synthetic image")
}

#[test]
fn thumbnail_access_updates_lru() {
    let mut cache = ThumbnailCache::default();
    for frame in 0..MAX_THUMBNAIL_ENTRIES {
        assert!(cache.insert(key(frame), image(frame), 1).is_empty());
    }
    assert!(cache.get(&key(0)).is_some());

    let retired = cache.insert(key(MAX_THUMBNAIL_ENTRIES), image(64), 1);
    assert_eq!(retired.len(), 1);
    assert!(cache.contains(&key(0)));
    assert!(!cache.contains(&key(1)));
    assert_eq!(cache.high_water_entries(), MAX_THUMBNAIL_ENTRIES);
}

#[test]
fn byte_limit_evicts_independently_of_entry_limit() {
    let mut cache = ThumbnailCache::default();
    let first = image(0);
    cache.insert(key(0), first.clone(), MAX_THUMBNAIL_BYTES - 10);
    let retired = cache.insert(key(1), image(1), 11);

    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes(), 11);
    assert_eq!(retired.len(), 1);
    assert!(Arc::ptr_eq(&retired[0], &first));
    assert_eq!(cache.high_water_bytes(), MAX_THUMBNAIL_BYTES - 10);
}

#[test]
fn oversized_input_is_returned_without_flushing_cache() {
    let mut cache = ThumbnailCache::default();
    let resident = image(0);
    cache.insert(key(0), resident.clone(), 8);
    let oversized = image(1);
    let retired = cache.insert(key(1), oversized.clone(), MAX_THUMBNAIL_BYTES + 1);

    assert_eq!(retired.len(), 1);
    assert!(Arc::ptr_eq(&retired[0], &oversized));
    assert!(cache.contains(&key(0)));
    assert!(!cache.contains(&key(1)));
    assert_eq!(cache.bytes(), 8);
}

#[test]
fn replacement_retires_old_image_and_accounts_new_size() {
    let mut cache = ThumbnailCache::default();
    let old = image(0);
    cache.insert(key(0), old.clone(), 100);
    let replacement = image(1);
    let retired = cache.insert(key(0), replacement.clone(), 40);

    assert_eq!(retired.len(), 1);
    assert!(Arc::ptr_eq(&retired[0], &old));
    assert!(Arc::ptr_eq(&cache.get(&key(0)).unwrap(), &replacement));
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.bytes(), 40);
    assert_eq!(cache.high_water_bytes(), 100);
}

#[test]
fn key_separates_revision_generation_scale_frame_and_media_inventory() {
    let base = ThumbnailKey::new(identity("r1", 1), 0.25, 4);
    let revision = ThumbnailKey::new(identity("r2", 1), 0.25, 4);
    let generation = ThumbnailKey::new(identity("r1", 2), 0.25, 4);
    let scale = ThumbnailKey::new(identity("r1", 1), 0.5, 4);
    let frame = ThumbnailKey::new(identity("r1", 1), 0.25, 5);
    let mut media = base.clone();
    media.media_hash = "different-media".into();

    assert_eq!(base.backend, "cpu");
    assert_eq!(base.media_hash, "r1");
    for distinct in [&revision, &generation, &scale, &frame, &media] {
        assert_ne!(&base, distinct);
    }
}

#[test]
fn clear_returns_every_image_and_resets_current_accounting() {
    let mut cache = ThumbnailCache::default();
    let images = [image(0), image(1), image(2)];
    for (frame, image) in images.iter().enumerate() {
        cache.insert(key(frame), image.clone(), frame + 1);
    }

    let retired = cache.clear();
    assert_eq!(retired.len(), images.len());
    assert!(
        images
            .iter()
            .all(|image| retired.iter().any(|item| Arc::ptr_eq(item, image)))
    );
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
    assert_eq!(cache.bytes(), 0);
    assert_eq!(cache.high_water_entries(), 3);
    assert_eq!(cache.high_water_bytes(), 6);
}

#[test]
#[ignore = "requires compatible SDK_ACTIVE or SDK_BUNDLE; exercises real compiled timeline and thumbnail destinations"]
fn real_compiled_tracks_and_thumbnail_seek_destinations() {
    use fframes_studio::preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent};
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, Instant},
    };
    use studio_engine::{
        Controller, JobKind, JobResult, PreviewState, TimelineModel, TimelineSelection,
        TimelineViewport, app_paths::AppPaths,
    };
    let temp = tempfile::tempdir().unwrap();
    let sdk = if let Some(bundle) = std::env::var_os("SDK_BUNDLE") {
        let bundle = PathBuf::from(bundle);
        let manifest = studio_sdk::CompatibilityManifest::from_json_str(
            &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
        )
        .unwrap();
        let artifacts: Vec<_> = manifest
            .artifacts
            .iter()
            .map(|a| (a.clone(), bundle.join(a.url.trim_start_matches("file://"))))
            .collect();
        let home = std::env::var_os("TIMELINE_NATIVE_SDK_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| temp.path().join("sdk"));
        studio_sdk::SdkInstaller::new(home)
            .install_from_local_artifacts(&manifest, &artifacts)
            .unwrap()
    } else {
        PathBuf::from(std::env::var_os("SDK_ACTIVE").expect("SDK_ACTIVE or SDK_BUNDLE"))
    };
    let manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(sdk.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    let root = std::env::var_os("TIMELINE_NATIVE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| temp.path().join("video"));
    preview_fixture::create(&root, &manifest);
    let data = std::env::var_os("TIMELINE_NATIVE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| temp.path().join("history"));
    let paths = AppPaths::new(data).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let checkpoint = controller.state().accepted().clone();
    let coordinator = PreviewCoordinator::new(controller.processes.sub_manager());
    let tag = controller.begin_job(JobKind::Build).unwrap();
    let mut state = PreviewState::default();
    state.begin(tag.clone());
    coordinator.build(BuildSpec {
        project: controller.project.clone(),
        sdk: sdk.clone(),
        compatibility: manifest,
        builds: paths.builds(),
        tag,
        compiler: controller.operation_processes(),
        worker: controller.processes.sub_manager(),
        service: fframes_studio::worker_project::shared_build_service(),
    });
    let deadline = Instant::now() + Duration::from_secs(180);
    let ready = loop {
        let event = coordinator.events();
        if let Some(tag) = event.compiled {
            controller
                .complete(&tag, JobResult::Built(tag.base_source.clone()))
                .unwrap();
        }
        assert!(event.error.is_none(), "{:?}", event.error);
        if let Some(ready) = event.ready {
            break ready;
        }
        assert!(Instant::now() < deadline, "real fixture readiness deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    // Core resolves each scene from the previous accumulated duration, subtracting next overlap.
    // 60 + (60 - 15) + 60 = 165, with overlay extending across both neighbours.
    assert_eq!(ready.timeline.total_frames, 165);
    let model = TimelineModel::new(ready.timeline.clone()).unwrap();
    assert_eq!(model.report().scenes.len(), 3);
    assert_eq!(
        model
            .report()
            .scenes
            .iter()
            .map(|s| (s.start_frame, s.end_frame))
            .collect::<Vec<_>>(),
        vec![(0, 60), (45, 135), (105, 165)]
    );
    assert_eq!(model.report().scenes[0].name, model.report().scenes[2].name);
    assert_ne!(
        model.report().scenes[0].instance_id,
        model.report().scenes[2].instance_id
    );
    // Core's 44.1kHz timeline rounds 0.125s to sample 5513, not video frame 4.
    assert_eq!(model.report().audio_tracks[0].start_seconds, 5513. / 44100.);
    let overlap = model.report().scenes[1].start_frame;
    assert_eq!(model.scene_hits(overlap), vec![0, 1]);
    let mut selection = TimelineSelection::default();
    selection.cycle_scene_at(overlap, &model);
    assert_eq!(
        selection.scene_id.as_ref(),
        Some(&model.report().scenes[0].instance_id)
    );
    selection.cycle_scene_at(overlap, &model);
    assert_eq!(
        selection.scene_id.as_ref(),
        Some(&model.report().scenes[1].instance_id)
    );
    assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
    state.install(&ready, controller.state()).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(600., &model);
    viewport.fit(&model);
    let samples = viewport.thumbnail_frames(&model);
    let scale = 0.125;
    coordinator.thumbnails(
        samples
            .iter()
            .map(|frame| ThumbnailKey::new(ready.identity().clone(), scale, *frame))
            .collect(),
    );
    let mut cache = ThumbnailCache::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    // Wait until thumbnail work has run, then give main preview an asymmetric latest seek.
    loop {
        let event = coordinator.events();
        assert!(
            event.frame.is_none(),
            "thumbnail cannot become a main preview frame"
        );
        if let Some((key, frame)) = event.thumbnail {
            assert_eq!(frame.response.frame_index, key.frame_index);
            assert!(
                !state.accepts_frame(&frame),
                "thumbnail must not have the main seek/scale tag"
            );
            let image =
                frame_image::create_render_image(&frame.response.header, &frame.pixels).unwrap();
            assert!(cache.insert(key, image, frame.pixels.len()).is_empty());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let serial = state.seek(137, state.scale()).unwrap();
    coordinator.seek(SeekIntent {
        identity: ready.identity().clone(),
        serial,
        position: 137,
        scale: state.scale(),
    });
    loop {
        let event = coordinator.events();
        if let Some(frame) = event.frame {
            assert!(state.accepts_frame(&frame));
            assert_eq!(frame.response.frame_index, 137);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state.position(), 137);
    assert_eq!(controller.state().accepted(), &checkpoint);
    coordinator.close();
    drop(coordinator);
    controller.close().unwrap();
    assert_eq!(controller.processes.active_count(), 0);
    // The final preview owns its PCM/materialization independently of the worker.
    drop(ready);
    // Completed builds stay cached by the shared service until the project closes (the
    // product close path releases them); do the same before checking for leaks.
    fframes_studio::worker_project::shared_build_service().close_project(&String::from(
        controller.project.manifest.project_id.clone(),
    ));
    fn no_leased_trees(path: &std::path::Path) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert!(
                    !path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("build-")
                );
                no_leased_trees(&path);
            }
        }
    }
    // The shared target cache is retained, but every immutable project/mix lease is removed.
    no_leased_trees(&paths.builds());
    // Native evidence reopens the exact fixture through the ordinary Recent/Open route.
    if std::env::var_os("TIMELINE_NATIVE_ROOT").is_some() {
        println!("Native fixture: {}", root.display());
    }
}
