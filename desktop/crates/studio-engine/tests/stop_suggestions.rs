//! Public scan, seek, cancellation, layout fencing and explicit acceptance behavior.
#[path = "support/build_fixture.rs"]
mod build_fixture;

use fframes_studio_protocol::{StopSuggestions, SuggestedSlideStop};
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    Controller, JobKind, JobResult, ReadyPreview,
    app_paths::AppPaths,
    services::{
        build_service::{BuildLimits, BuildService},
        preview_coordinator::{
            BuildSpec, PreviewCoordinator, PreviewProps, PreviewTiming, SeekIntent,
        },
    },
    slides::{SlideStop, SlidesCommand, SlidesDoc, StopAt, suggest},
    timing::TimelineCommand,
};

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "preview condition timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn requests(path: &std::path::Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn install(
    c: &mut Controller,
    p: &PreviewCoordinator,
    sdk: &std::path::Path,
    paths: &AppPaths,
    service: &BuildService,
) -> Arc<ReadyPreview> {
    let timing = c.timeline_snapshot();
    p.set_timing(PreviewTiming {
        json: timing.timing_json,
        generation: timing.generation,
    });
    let (json, generation) = c.props_for_preview();
    p.set_props(PreviewProps { json, generation });
    let tag = c.begin_job(JobKind::Build).unwrap();
    p.build(BuildSpec {
        project: c.project.clone(),
        sdk: sdk.into(),
        compatibility: build_fixture::manifest(),
        builds: paths.builds(),
        tag,
        compiler: c.operation_processes(),
        worker: c.processes.sub_manager(),
        service: service.clone(),
    });
    let mut ready = None;
    wait(|| {
        let events = p.events();
        if let Some(tag) = events.compiled {
            c.complete(&tag, JobResult::Built(tag.base_source.clone()))
                .unwrap();
        }
        assert!(events.error.is_none(), "{:?}", events.error);
        ready = events.ready;
        ready.is_some()
    });
    let ready = ready.unwrap();
    c.learn_timeline(ready.tag(), &ready.timeline);
    assert!(p.commit(ready.identity().clone(), ready.seek_serial));
    ready
}

#[test]
fn scan_interleaves_seeks_cancels_fences_generations_and_accepts_only_on_command() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    build_fixture::create_project(&root);
    let log = temp.path().join("requests.jsonl");
    let config = serde_json::json!({"frames":600,"timing":true,"suggest_delay_ms":200,"props":{"fields":[],"values":{}},
        "request_log":log, "suggestions":[
            {"scene":"video","origin":"bullets","seconds":1.0,"label":"First bullet","notes":"Explain it"},
            {"scene":"video","origin":"bullets","seconds":2.0,"label":"Second bullet","notes":null}]});
    fs::write(root.join(build_fixture::FAKE_CONFIG), config.to_string()).unwrap();
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let mut c = Controller::open(&root, &paths).unwrap();
    let owner = ProcessTreeManager::new();
    let service = BuildService::new(
        owner.clone(),
        build_fixture::FakeCompiler::new(true),
        BuildLimits::default(),
    );
    let sdk = build_fixture::fake_sdk(temp.path());
    let p = PreviewCoordinator::new(owner.sub_manager());
    let ready = install(&mut c, &p, &sdk, &paths, &service);
    wait(|| p.stop_scan().is_some());
    assert!(!root.join("slides.json").exists());
    p.scan_stops();
    wait(|| {
        requests(&log)
            .iter()
            .any(|request| request["type"] == "SuggestStops")
    });
    p.seek(SeekIntent {
        identity: ready.identity().clone(),
        serial: 1,
        position: 73,
        scale: 1.0,
    });
    wait(|| {
        p.events()
            .frame
            .is_some_and(|frame| frame.response.frame_index == 73)
    });
    wait(|| p.stop_scan().is_some_and(|scan| !scan.running));
    let sent: Vec<_> = requests(&log)
        .into_iter()
        .filter(|request| request["type"] == "SuggestStops")
        .collect();
    assert_eq!(sent.len(), 3);
    assert!(sent[0]["reset"].as_bool().unwrap());
    assert!(sent[1..].iter().all(|request| request["reset"] == false));
    assert!(
        sent.iter()
            .all(|request| request["frames"].as_array().unwrap().len() <= 16)
    );
    let log_entries = requests(&log);
    let seek_index = log_entries
        .iter()
        .position(|request| request["type"] == "ScaledFrame" && request["frame_index"] == 73)
        .unwrap();
    let last_batch = log_entries
        .iter()
        .rposition(|request| request["type"] == "SuggestStops")
        .unwrap();
    assert!(
        seek_index < last_batch,
        "seeks dispatch before the next queued batch"
    );
    assert!(
        !root.join("slides.json").exists(),
        "a scan must never silently write"
    );
    let scan = p.stop_scan().unwrap();
    assert_eq!(scan.completed, scan.total);
    let proposals = suggest::proposals(&scan.suggestions, &scan.timeline, &SlidesDoc::default());
    assert_eq!(
        proposals.iter().map(|p| p.video_frame).collect::<Vec<_>>(),
        [30, 60]
    );
    let accept = SlidesCommand::AcceptSuggestions {
        identity: ready.identity().clone(),
        timing_generation: ready.timeline.timing_generation,
        props_generation: scan.props_generation,
        stops: proposals
            .iter()
            .map(|proposal| proposal.stop.clone())
            .collect(),
    };
    let mut stale_identity = ready.identity().clone();
    stale_identity.worker_generation += 1;
    assert!(
        c.slides_command(SlidesCommand::AcceptSuggestions {
            identity: stale_identity,
            timing_generation: ready.timeline.timing_generation,
            props_generation: scan.props_generation,
            stops: vec![proposals[0].stop.clone()],
        })
        .is_err()
    );
    assert!(
        c.slides_command(SlidesCommand::AcceptSuggestions {
            identity: ready.identity().clone(),
            timing_generation: ready.timeline.timing_generation + 1,
            props_generation: scan.props_generation,
            stops: vec![proposals[0].stop.clone()],
        })
        .is_err()
    );
    assert!(
        !root.join("slides.json").exists(),
        "stale proposals never write"
    );
    c.slides_command(accept.clone()).unwrap();
    assert_eq!(c.slides_snapshot().doc.unwrap().stops.len(), 2);
    c.slides_command(accept.clone()).unwrap();
    assert_eq!(
        c.slides_snapshot().doc.unwrap().stops.len(),
        2,
        "accepting twice is idempotent"
    );
    c.timeline_command(TimelineCommand::Undo).unwrap();
    assert!(
        c.slides_snapshot().doc.unwrap().stops.is_empty(),
        "accept all is one undo step"
    );

    p.scan_stops();
    wait(|| {
        requests(&log)
            .iter()
            .filter(|request| request["type"] == "SuggestStops")
            .count()
            > 3
    });
    p.cancel_stop_scan();
    std::thread::sleep(Duration::from_millis(250));
    let cancelled = p.stop_scan().unwrap();
    assert!(
        !cancelled.running && cancelled.suggestions.stops.is_empty(),
        "in-flight reply was discarded"
    );
    let batches = requests(&log)
        .iter()
        .filter(|request| request["type"] == "SuggestStops")
        .count();
    p.scan_stops();
    wait(|| {
        requests(&log)
            .iter()
            .filter(|request| request["type"] == "SuggestStops")
            .count()
            > batches
    });
    p.set_timing(PreviewTiming {
        json: None,
        generation: 9,
    });
    assert!(
        p.stop_scan().is_none(),
        "generation changes fence immediately"
    );
    wait(|| {
        p.stop_scan()
            .is_some_and(|scan| scan.timeline.timing_generation == 9)
    });
    let fresh = p.stop_scan().unwrap();
    assert!(!fresh.running && fresh.suggestions.stops.is_empty());

    let batches = requests(&log)
        .iter()
        .filter(|request| request["type"] == "SuggestStops")
        .count();
    p.scan_stops();
    wait(|| {
        requests(&log)
            .iter()
            .filter(|request| request["type"] == "SuggestStops")
            .count()
            > batches
    });
    c.props_command(studio_engine::props::PropsCommand::Set {
        identity: fframes_studio_protocol::EditorObjectIdentity {
            scene_instance_key: "s0".into(),
            component_key: "Bullets".into(),
            object_key: "reveal".into(),
            repeat_key: "main".into(),
        },
        field: "delay".into(),
        value: Some(serde_json::json!(2)),
    })
    .unwrap();
    assert!(
        c.slides_command(accept)
            .unwrap_err()
            .to_string()
            .contains("props"),
        "a queued acceptance from before a props edit must be refused"
    );
    assert!(c.slides_snapshot().doc.unwrap().stops.is_empty());
    p.set_props(PreviewProps {
        json: None,
        generation: c.props_for_preview().1,
    });
    std::thread::sleep(Duration::from_millis(250));
    let changed_props = p.stop_scan().unwrap();
    assert!(
        !changed_props.running && changed_props.suggestions.stops.is_empty(),
        "props changes fence in-flight suggestions too"
    );

    fs::write(root.join(build_fixture::FAKE_CONFIG), r#"{"frames":90}"#).unwrap();
    c.reconcile().unwrap();
    install(&mut c, &p, &sdk, &paths, &service);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        p.stop_scan().is_none(),
        "older workers hide all suggestion controls"
    );
    p.close();
    owner.shutdown(Duration::ZERO);
}

#[test]
fn mapping_honors_trims_overlaps_accepted_frames_and_document_bounds() {
    // A real public worker timeline is exercised above; use its JSON shape here to
    // describe a trimmed overlap that cannot be represented by the fake worker.
    let identity = serde_json::json!({"project_id":"p","open_session":"s","source_revision":"r","worker_generation":1});
    let scene = |key: &str, start: usize, end: usize, trim: usize| {
        serde_json::json!({
        "instance_id":key,"editor_instance_key":key,"index":start,"name":key,"full_name":key,
        "start_frame":start,"end_frame":end,"start_seconds":start as f32/30.0,"end_seconds":end as f32/30.0,
        "clip":{"timing_key":key,"clip_index":0,"trim_start_frames":trim,"trim_end_frames":0,
            "scene_index":0,"scene_frames":300}})
    };
    let timeline = serde_json::from_value(serde_json::json!({"envelope":{"contract_version":1,"identity":identity,"request_id":1},
        "fps":30,"width":8,"height":4,"total_frames":300,"duration_seconds":10.0,"timing_generation":0,
        "audio_tracks":[],"scenes":[scene("intro",0,180,30),scene("main",150,300,60)]})).unwrap();
    let stop = |scene: &str, seconds| SuggestedSlideStop {
        scene: scene.into(),
        origin: "list".into(),
        seconds,
        label: None,
        notes: None,
    };
    let suggestions = StopSuggestions {
        stops: vec![
            stop("video", 5.5),
            stop("main", 2.5),
            stop("intro", 0.5),
            stop("unknown", 1.0),
            stop("video", f32::NAN),
            stop("video", 15.0),
        ],
        truncated: false,
    };
    let found = suggest::proposals(&suggestions, &timeline, &SlidesDoc::default());
    assert_eq!(
        found.len(),
        1,
        "trimmed, duplicate, invalid and missing stops are omitted"
    );
    assert_eq!(
        (
            found[0].stop.scene.as_str(),
            found[0].stop.at,
            found[0].video_frame
        ),
        ("main", StopAt::Frame(75), 165)
    );
    let accepted = suggest::accepting(&SlidesDoc::default(), &found).unwrap();
    assert!(suggest::proposals(&suggestions, &timeline, &accepted).is_empty());
    let asymmetric = StopSuggestions {
        stops: vec![stop("video", 0.41), stop("main", 2.41), stop("video", 0.0)],
        truncated: false,
    };
    let rounded_up = suggest::proposals(&asymmetric, &timeline, &SlidesDoc::default());
    assert_eq!(
        rounded_up
            .iter()
            .map(|proposal| (proposal.stop.at, proposal.video_frame))
            .collect::<Vec<_>>(),
        [(StopAt::Frame(43), 13), (StopAt::Frame(73), 163)],
        "fractional reveals must be fully complete: 0.41s at 30fps holds on 13, never 12; zero is dropped"
    );
    let full = SlidesDoc {
        stops: vec![
            SlideStop {
                scene: "main".into(),
                at: StopAt::Frame(1),
                title: None,
                notes: None
            };
            300
        ],
        ..SlidesDoc::default()
    };
    assert!(
        suggest::accepting(&full, &found)
            .unwrap_err()
            .contains("limit of 300")
    );
    let mut large = found[0].clone();
    large.stop.notes = Some("n".repeat(8 * 1024));
    assert!(
        suggest::accepting(&SlidesDoc::default(), &vec![large; 65])
            .unwrap_err()
            .contains("bytes"),
        "the encoded document has a separate byte budget"
    );
    let (frames, limited) = suggest::sample_frames(&timeline);
    assert!(!limited && frames.contains(&150) && frames.contains(&0));
    let mut off_grid = timeline.clone();
    off_grid.scenes[1].start_frame = 151;
    assert!(
        suggest::sample_frames(&off_grid).0.contains(&151),
        "clip starts are sampled even off the half-second grid"
    );
    let mut scene_less = timeline.clone();
    scene_less.scenes.clear();
    let root_suggestions = StopSuggestions {
        stops: vec![stop("video", 0.41)],
        truncated: false,
    };
    let root_proposals = suggest::proposals(&root_suggestions, &scene_less, &SlidesDoc::default());
    assert_eq!(
        (
            root_proposals[0].stop.scene.as_str(),
            root_proposals[0].stop.at
        ),
        ("video", StopAt::Frame(13))
    );
    let root_doc = suggest::accepting(&SlidesDoc::default(), &root_proposals).unwrap();
    let (resolved, problems) = studio_engine::slides::resolve::resolve(&root_doc, &scene_less);
    assert!(problems.is_empty());
    assert_eq!(resolved[0].video_frame, 13);
    assert!(suggest::proposals(&root_suggestions, &scene_less, &root_doc).is_empty());
    let mut old_worker = timeline.clone();
    old_worker.scenes[0].clip = None;
    assert!(
        suggest::proposals(&root_suggestions, &old_worker, &SlidesDoc::default()).is_empty(),
        "virtual root clips never mask missing metadata on real scenes"
    );
    let mut long = timeline;
    long.total_frames = usize::MAX;
    let (frames, limited) = suggest::sample_frames(&long);
    assert!(limited);
    assert_eq!(frames.len(), suggest::MAX_SCAN_SAMPLES);
}
