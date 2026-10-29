//! Slide stops of an open project: `slides.json` edits resolved against the timeline the
//! person sees, the shared timing and slides undo history, outside writes, crashes, agent
//! candidates and task undo.
#![cfg(any(target_os = "linux", target_os = "macos", windows))]

#[path = "support/tx_fixture.rs"]
mod tx_fixture;

use std::{fs, sync::Arc};

use fframes_studio_protocol::{
    PREVIEW_CONTRACT_VERSION, PreviewClipTiming, PreviewEnvelope, PreviewSceneInfo,
    PreviewTimelineResponse,
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    Boundary, CompletionOutcome, Controller, Fault, JobKind, JobResult, OperationTag,
    build_materialization::MaterializedBuild,
    candidate_validation::FailureKind,
    preview_identity,
    services::{
        build_service::{BuildLimits, BuildService, CompileRequest, Compiler},
        candidate_runner::{CandidateRunConfig, RunScopes, run_candidate_validation},
    },
    slides::{SlideStop, SlidesCommand, SlidesDoc, SlidesSnapshot, StopAt},
    timing::{TimelineCommand, TimingDoc, TrimEdge, file::WriteStep},
};
use studio_project::revision::{SLIDES_FILE, TIMING_FILE};
use studio_sdk::CompatibilityManifest;
use tx_fixture::Fx;

/// Scenes `intro` (30 frames), `main` (60) and `outro` (30), in code order.
const SCENES: [(&str, usize); 3] = [("intro", 30), ("main", 60), ("outro", 30)];

/// A clip as the worker lays it out: `(scene index, trim_start, trim_end)`.
type Clip = (usize, usize, usize);

/// The timeline a worker reports for `clips` played end to end under `generation`.
fn laid_out(tag: &OperationTag, generation: u64, clips: &[Clip]) -> PreviewTimelineResponse {
    let mut start = 0;
    let scenes = clips
        .iter()
        .enumerate()
        .map(|(index, &(scene, trim_start, trim_end))| {
            let (key, frames) = SCENES[scene];
            let played = frames - trim_start - trim_end;
            let info = PreviewSceneInfo {
                instance_id: format!("scene-{index}"),
                editor_instance_key: Some(key.into()),
                index,
                name: key.into(),
                full_name: key.into(),
                start_frame: start,
                end_frame: start + played,
                start_seconds: start as f32 / 30.,
                end_seconds: (start + played) as f32 / 30.,
                clip: Some(PreviewClipTiming {
                    timing_key: key.into(),
                    clip_index: 0,
                    trim_start_frames: trim_start,
                    trim_end_frames: trim_end,
                    scene_index: Some(scene),
                    scene_frames: Some(frames),
                }),
            };
            start += played;
            info
        })
        .collect();
    PreviewTimelineResponse {
        envelope: PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: preview_identity(tag),
            request_id: 1,
        },
        fps: 30,
        width: 1920,
        height: 1080,
        total_frames: start,
        duration_seconds: start as f32 / 30.,
        scenes,
        audio_tracks: vec![],
        timing_generation: generation,
    }
}

/// A finished build of the current source whose preview laid out the code order under
/// the session's timing generation.
fn built(controller: &mut Controller) -> OperationTag {
    let tag = controller.begin_job(JobKind::Build).unwrap();
    controller
        .complete(&tag, JobResult::Built(tag.base_source.clone()))
        .unwrap();
    let generation = controller.timeline_snapshot().generation;
    controller.learn_timeline(
        &tag,
        &laid_out(&tag, generation, &[(0, 0, 0), (1, 0, 0), (2, 0, 0)]),
    );
    tag
}

/// What the engine hears from a retime of the displayed preview (`PreviewNews::retimed`).
fn retimed(controller: &mut Controller, tag: &OperationTag, clips: &[Clip]) {
    let generation = controller.timeline_snapshot().generation;
    controller.learn_timeline(tag, &laid_out(tag, generation, clips));
}

fn frames(snapshot: &SlidesSnapshot) -> Vec<usize> {
    snapshot.stops.iter().map(|stop| stop.video_frame).collect()
}

fn stop(scene: &str, at: StopAt) -> SlideStop {
    SlideStop {
        scene: scene.into(),
        at,
        title: None,
        notes: None,
    }
}

fn live(root: &std::path::Path) -> SlidesDoc {
    SlidesDoc::parse(&fs::read_to_string(root.join(SLIDES_FILE)).unwrap()).unwrap()
}

fn opened(f: &Fx) -> (Controller, OperationTag) {
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let tag = built(&mut controller);
    (controller, tag)
}

#[test]
fn a_rebuild_cannot_resolve_slides_using_the_previous_previews_layout() {
    let f = tx_fixture::fixture();
    let (mut c, _) = opened(&f);
    assert_eq!(frames(&c.slides_snapshot()), [29, 89, 119]);
    fs::write(f.root.join("notes.txt"), "changed source\n").unwrap();
    c.reconcile().unwrap();
    let tag = c.begin_job(JobKind::Build).unwrap();
    c.complete(&tag, JobResult::Built(tag.base_source.clone()))
        .unwrap();
    assert!(c.resolved_slides().is_err(), "the new preview is not ready");
    assert!(
        c.slides_command(SlidesCommand::MaterializeDefaultStops)
            .is_err()
    );
    // Same duration, different scene order: a duration check cannot catch stale stops.
    retimed(&mut c, &tag, &[(1, 0, 0), (0, 0, 0), (2, 0, 0)]);
    assert_eq!(frames(&c.slides_snapshot()), [59, 89, 119]);
}

#[test]
fn slides_default_to_clip_ends_and_follow_a_trim_without_a_rebuild() {
    let f = tx_fixture::fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let before = c.slides_snapshot();
    assert!(before.stops.is_empty());
    assert!(
        before.unavailable.is_some(),
        "nothing resolves before a preview"
    );
    let tag = built(&mut c);
    assert_eq!(
        frames(&c.slides_snapshot()),
        [29, 89, 119],
        "one stop per clip end"
    );

    c.slides_command(SlidesCommand::AddStop(stop("main", StopAt::Frame(40))))
        .unwrap();
    assert_eq!(frames(&c.slides_snapshot()), [70, 119]);
    let generation = c.state().generation();
    let build = c.state().build_source().clone();

    c.timeline_command(TimelineCommand::Trim {
        clip: 1,
        edge: TrimEdge::Start,
        frames: 10,
    })
    .unwrap();
    // Until the displayed preview is laid out again, the old layout is not used.
    let stale = c.slides_snapshot();
    assert!(
        stale.stops.is_empty() && stale.unavailable.is_some(),
        "{stale:?}"
    );
    retimed(&mut c, &tag, &[(0, 0, 0), (1, 10, 0), (2, 0, 0)]);
    let snapshot = c.slides_snapshot();
    assert_eq!(
        frames(&snapshot),
        [60, 109],
        "scene frame 40 is 30 frames into main"
    );
    assert!(snapshot.problems.is_empty(), "{:?}", snapshot.problems);
    // Neither edit touched the build.
    assert_eq!(c.state().generation(), generation);
    assert_eq!(c.state().build_source(), &build);

    // A trim that cuts the stop's frame leaves it unresolved, never moved.
    c.timeline_command(TimelineCommand::Trim {
        clip: 1,
        edge: TrimEdge::Start,
        frames: 45,
    })
    .unwrap();
    retimed(&mut c, &tag, &[(0, 0, 0), (1, 45, 0), (2, 0, 0)]);
    let snapshot = c.slides_snapshot();
    assert_eq!(frames(&snapshot), [74]);
    assert!(
        snapshot.problems[0].is_unresolved(),
        "{:?}",
        snapshot.problems
    );
}

#[test]
fn slides_undo_goes_back_through_marker_and_trim_in_order() {
    let f = tx_fixture::fixture();
    let (mut c, tag) = opened(&f);
    c.slides_command(SlidesCommand::AddStop(stop("main", StopAt::Frame(40))))
        .unwrap();
    c.timeline_command(TimelineCommand::Trim {
        clip: 0,
        edge: TrimEdge::End,
        frames: 5,
    })
    .unwrap();
    retimed(&mut c, &tag, &[(0, 0, 5), (1, 0, 0), (2, 0, 0)]);
    // A marker drag: one undo step and one write.
    c.slides_command(SlidesCommand::BeginGesture).unwrap();
    for at in 41..=50 {
        c.slides_command(SlidesCommand::MoveStop {
            stop: 0,
            scene: "main".into(),
            at: StopAt::Frame(at),
        })
        .unwrap();
    }
    assert_eq!(
        live(&f.root).stops[0].at,
        StopAt::Frame(40),
        "nothing written mid-drag"
    );
    // The timeline is not edited inside a marker drag.
    assert!(
        c.timeline_command(TimelineCommand::Trim {
            clip: 0,
            edge: TrimEdge::End,
            frames: 6,
        })
        .is_err()
    );
    c.slides_command(SlidesCommand::EndGesture).unwrap();
    assert_eq!(live(&f.root).stops[0].at, StopAt::Frame(50));
    assert!(c.timeline_snapshot().can_undo);

    // First undo: the marker, not the trim.
    c.timeline_command(TimelineCommand::Undo).unwrap();
    assert_eq!(live(&f.root).stops[0].at, StopAt::Frame(40));
    assert_eq!(
        c.timeline_snapshot().clips[0].frames(),
        25,
        "the trim stands"
    );
    // Second undo: the trim; the marker stays.
    c.timeline_command(TimelineCommand::Undo).unwrap();
    assert_eq!(c.timeline_snapshot().clips[0].frames(), 30);
    assert_eq!(live(&f.root).stops[0].at, StopAt::Frame(40));
    // Third: the stop's creation.
    c.timeline_command(TimelineCommand::Undo).unwrap();
    assert!(live(&f.root).stops.is_empty());
    assert!(!c.timeline_snapshot().can_undo);
    // Redo goes forward in the same order.
    c.timeline_command(TimelineCommand::Redo).unwrap();
    c.timeline_command(TimelineCommand::Redo).unwrap();
    assert_eq!(live(&f.root).stops.len(), 1);
    assert_eq!(c.timeline_snapshot().clips[0].frames(), 25);
    c.timeline_command(TimelineCommand::Redo).unwrap();
    assert_eq!(live(&f.root).stops[0].at, StopAt::Frame(50));
}

#[test]
fn slides_outside_write_is_kept_and_clears_only_the_slides_steps() {
    let f = tx_fixture::fixture();
    let (mut c, _) = opened(&f);
    c.timeline_command(TimelineCommand::Move { clip: 2, to: 0 })
        .unwrap();
    c.slides_command(SlidesCommand::AddStop(stop("intro", StopAt::End)))
        .unwrap();
    let outside = "{\"version\":1,\"stops\":[{\"scene\":\"main\",\"at\":12}]}\n";
    fs::write(f.root.join(SLIDES_FILE), outside).unwrap();

    // An edit against the old file is refused and the outside version reloaded.
    let error = c
        .slides_command(SlidesCommand::RemoveStop { stop: 0 })
        .unwrap_err()
        .to_string();
    assert!(error.contains("changed outside"), "{error}");
    assert_eq!(
        fs::read_to_string(f.root.join(SLIDES_FILE)).unwrap(),
        outside
    );
    let snapshot = c.slides_snapshot();
    assert_eq!(snapshot.doc.unwrap().stops[0].scene, "main");
    assert!(snapshot.notice.is_some());

    // The slides step is gone; the timing step is the one undone.
    c.timeline_command(TimelineCommand::Undo).unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join(SLIDES_FILE)).unwrap(),
        outside
    );
    let timing =
        TimingDoc::from_json(&fs::read_to_string(f.root.join(TIMING_FILE)).unwrap()).unwrap();
    assert_eq!(timing, TimingDoc::default(), "the code order again");
    assert_eq!(c.timeline_snapshot().clips[0].key, "intro");
    assert!(!c.timeline_snapshot().can_undo);

    // An invalid outside file disables editing with its reason; the file stands.
    fs::write(
        f.root.join(SLIDES_FILE),
        "{\"version\":1,\"stops\":[],\"x\":1}",
    )
    .unwrap();
    c.reconcile().unwrap();
    let snapshot = c.slides_snapshot();
    assert!(snapshot.unavailable.is_some() && snapshot.doc.is_none());
    let error = c
        .slides_command(SlidesCommand::AddStop(stop("intro", StopAt::End)))
        .unwrap_err()
        .to_string();
    assert!(error.contains(SLIDES_FILE), "{error}");
}

#[test]
fn slides_crash_between_displace_and_publish_is_recovered_on_open() {
    let f = tx_fixture::fixture();
    let (mut c, _) = opened(&f);
    c.slides_command(SlidesCommand::AddStop(stop("intro", StopAt::End)))
        .unwrap();
    c.set_transaction_hooks(Arc::new(|boundary: &Boundary| {
        if matches!(boundary, Boundary::SlidesWrite(WriteStep::Displaced)) {
            Err(Fault::Crash)
        } else {
            Ok(())
        }
    }));
    assert!(
        c.slides_command(SlidesCommand::SetStopText {
            stop: 0,
            title: Some("Welcome".into()),
            notes: Some("Say hello.".into()),
        })
        .is_err()
    );
    assert!(
        !f.root.join(SLIDES_FILE).exists(),
        "the crash left no live file"
    );
    drop(c);

    let reopened = Controller::open(&f.root, &f.paths).unwrap();
    let doc = live(&f.root);
    assert_eq!(doc.stops[0].title.as_deref(), Some("Welcome"));
    assert_eq!(doc.stops[0].notes.as_deref(), Some("Say hello."));
    assert!(
        reopened
            .recovery_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("interrupted save")),
        "{:?}",
        reopened.recovery_notice
    );
}

/// Never reached: the slides document is checked before anything compiles.
struct NoCompiler;

impl Compiler for NoCompiler {
    fn compile(
        &self,
        _: &CompileRequest,
        _: &ProcessTreeManager,
    ) -> Result<Arc<MaterializedBuild>, String> {
        Err("the test compiler compiles nothing".into())
    }
}

#[test]
fn slides_agent_candidate_with_an_invalid_document_fails_validation() {
    let f = tx_fixture::fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let context = tx_fixture::start(&mut c, "add slides");
    fs::write(
        context.draft.join(SLIDES_FILE),
        "{\"version\":1,\"stops\":[{\"scene\":\"intro\",\"at\":\"later\"}]}",
    )
    .unwrap();
    let ticket = tx_fixture::ticket(&mut c, &context);
    let captured = c.agent_capture_candidate(ticket).unwrap();
    let config = CandidateRunConfig {
        service: BuildService::new(
            ProcessTreeManager::new(),
            Arc::new(NoCompiler),
            BuildLimits::default(),
        ),
        sdk: f.temp.path().join("sdk"),
        compatibility: CompatibilityManifest::default_linux_x64(),
        builds: f.temp.path().join("builds"),
    };
    let scopes = RunScopes {
        compiler: ProcessTreeManager::new(),
        worker: ProcessTreeManager::new(),
    };
    let run = run_candidate_validation(&captured, &c.checkpoints, &config, &scopes, 0, 0);
    assert!(!run.report.passed());
    let failure = run.report.failure().expect("a failure is recorded");
    assert_eq!(failure.kind, FailureKind::Source, "{failure:?}");
    let text = format!("{} {}", failure.summary, failure.output_tail);
    assert!(text.contains(SLIDES_FILE), "{text}");
}

#[test]
fn slides_task_undo_keeps_notes_edited_after_the_task_and_reverts_its_code() {
    let f = tx_fixture::fixture();
    let mut c = Controller::open(&f.root, &f.paths).unwrap();
    let (_, captured, report) = tx_fixture::validated(&f, &mut c, "add a slide", |draft| {
        fs::write(draft.join("notes.txt"), "edited by the agent\n").unwrap();
        fs::write(
            draft.join(SLIDES_FILE),
            "{\"version\":1,\"stops\":[{\"scene\":\"intro\",\"at\":\"end\",\"notes\":\"agent\"}]}\n",
        )
        .unwrap();
    });
    let CompletionOutcome::Applied(_) = c.complete_validated_task(&captured, &report).unwrap()
    else {
        panic!("the default policy applies");
    };
    assert_eq!(live(&f.root).stops[0].notes.as_deref(), Some("agent"));

    c.slides_command(SlidesCommand::SetStopText {
        stop: 0,
        title: None,
        notes: Some("mine".into()),
    })
    .unwrap();
    let undo = c.prepare_undo(None).unwrap();
    let undo_report = tx_fixture::passing_probe::passing_report(undo.captured());
    c.undo_task(&undo, &undo_report).unwrap();

    assert_eq!(
        fs::read_to_string(f.root.join("notes.txt")).unwrap(),
        "keep me\n",
        "the task's code edit was undone"
    );
    assert_eq!(
        live(&f.root).stops[0].notes.as_deref(),
        Some("mine"),
        "the person's notes stay"
    );
}
