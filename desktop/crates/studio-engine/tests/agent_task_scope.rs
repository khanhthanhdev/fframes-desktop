use fframes_studio_protocol::*;
use std::fs;
use studio_engine::{ScopeSelection, TaskScope, TaskScopeError, TimelineSelection};

fn scene(id: &str, index: usize, start_frame: usize, end_frame: usize) -> PreviewSceneInfo {
    PreviewSceneInfo {
        instance_id: format!("worker-7:{id}"),
        index,
        name: format!("Scene {id}"),
        full_name: format!("video::{id}"),
        start_frame,
        end_frame,
        start_seconds: start_frame as f32 / 30.0,
        end_seconds: end_frame as f32 / 30.0,
    }
}

fn timeline() -> PreviewTimelineResponse {
    PreviewTimelineResponse {
        envelope: PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: PreviewIdentity {
                project_id: "project-1".into(),
                open_session: "session-1".into(),
                source_revision: "a".repeat(64),
                worker_generation: 7,
            },
            request_id: 1,
        },
        fps: 30,
        width: 1920,
        height: 1080,
        total_frames: 60,
        duration_seconds: 2.0,
        scenes: vec![
            scene("intro", 0, 0, 20),
            scene("main", 1, 15, 45),
            scene("outro", 2, 45, 60),
        ],
        audio_tracks: Vec::new(),
    }
}

#[test]
fn scene_scope_freezes_instance_overlap_and_adjacent_boundaries() {
    let report = timeline();
    let mut selected = TimelineSelection::default();
    selected.select_scene(
        1,
        &studio_engine::TimelineModel::new(report.clone()).unwrap(),
    );

    let scope = TaskScope::from_timeline(&report, &selected).unwrap();
    assert!(
        matches!(scope.selection, ScopeSelection::Scene { ref instance_id, .. } if instance_id == "worker-7:main")
    );
    let compiled = scope.compiled.as_ref().unwrap();
    assert_eq!((compiled.start_frame, compiled.end_frame), (15, 45));
    assert_eq!(
        compiled
            .scenes
            .iter()
            .map(|scene| scene.instance_id.as_str())
            .collect::<Vec<_>>(),
        ["worker-7:intro", "worker-7:main", "worker-7:outro"]
    );
    for frame in [14, 15, 19, 20, 44, 45] {
        assert!(compiled.boundary_frames.contains(&frame));
    }
    assert_eq!(scope.source_revision, "a".repeat(64));
    assert!(scope.is_current_preview(&report.envelope.identity));
    scope.validate().unwrap();
}

#[test]
fn ranges_are_half_open_and_empty_ranges_require_no_frame_evidence() {
    let report = timeline();
    let model = studio_engine::TimelineModel::new(report.clone()).unwrap();
    let mut selected = TimelineSelection::default();
    selected.select_range(20, 10, &model);
    let scope = TaskScope::from_timeline(&report, &selected).unwrap();
    assert!(matches!(scope.selection, ScopeSelection::FrameRange));
    assert_eq!(
        (
            scope.compiled.as_ref().unwrap().start_frame,
            scope.compiled.as_ref().unwrap().end_frame
        ),
        (10, 20)
    );

    for cursor in [20, 45, 60] {
        selected.select_range(cursor, cursor, &model);
        let empty = TaskScope::from_timeline(&report, &selected).unwrap();
        assert_eq!(
            (
                empty.compiled.as_ref().unwrap().start_frame,
                empty.compiled.as_ref().unwrap().end_frame
            ),
            (cursor, cursor)
        );
        assert!(empty.compiled.as_ref().unwrap().boundary_frames.is_empty());
        assert!(empty.compiled.as_ref().unwrap().scenes.is_empty());
    }
}

#[test]
fn stale_scene_instances_and_inconsistent_identities_are_refused() {
    let report = timeline();
    let selected = TimelineSelection {
        scene_id: Some("old-worker:main".into()),
        range: Some(15..45),
    };
    assert_eq!(
        TaskScope::from_timeline(&report, &selected).unwrap_err(),
        TaskScopeError::MissingScene
    );

    let mut scope = TaskScope::from_timeline(&report, &TimelineSelection::default()).unwrap();
    scope.source_revision = "b".repeat(64);
    assert_eq!(scope.validate().unwrap_err(), TaskScopeError::Range);
}

/// A preview adopted from a promotion carries worker generation 0: a scope frozen against
/// it must still validate, and staleness is decided by full identity equality.
#[test]
fn a_scope_frozen_against_an_adopted_preview_with_generation_zero_is_valid() {
    let mut report = timeline();
    report.envelope.identity.worker_generation = 0;
    let model = studio_engine::TimelineModel::new(report.clone()).unwrap();
    let mut selected = TimelineSelection::default();
    selected.select_range(10, 25, &model);
    let scope = TaskScope::from_timeline(&report, &selected).unwrap();
    assert!(scope.is_current_preview(&report.envelope.identity));
    let mut other = report.envelope.identity.clone();
    other.worker_generation = 1;
    assert!(!scope.is_current_preview(&other));
}

#[test]
fn scope_round_trips_and_legacy_whole_project_has_no_compiled_identity() {
    let report = timeline();
    let scope = TaskScope::from_timeline(&report, &TimelineSelection::default()).unwrap();
    let encoded = serde_json::to_vec(&scope).unwrap();
    assert_eq!(
        serde_json::from_slice::<TaskScope>(&encoded).unwrap(),
        scope
    );

    let legacy = TaskScope::whole_project("project-1", "a".repeat(64));
    assert_eq!(legacy.label(), "Whole project");
    assert!(legacy.compiled.is_none());
    legacy.validate().unwrap();
}

#[test]
fn scene_source_candidates_are_bounded_exact_and_bound_to_inventory_hashes() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("src/a.rs"),
        "pub fn main() { helper(); }\npub fn helper() {}\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/b.rs"), "pub fn main() {}\n").unwrap();
    let inventory = studio_project::revision::SourceInventory::scan(temp.path()).unwrap();

    let mut report = timeline();
    report.envelope.identity.source_revision = inventory.revision.as_str().into();
    let model = studio_engine::TimelineModel::new(report.clone()).unwrap();
    let mut selected = TimelineSelection::default();
    selected.select_scene(1, &model);
    let mut scope = TaskScope::from_timeline(&report, &selected).unwrap();
    scope
        .resolve_scene_sources(temp.path(), &inventory.files)
        .unwrap();
    scope.validate().unwrap();

    let main = scope
        .scene_sources
        .iter()
        .find(|reference| reference.instance_id == "worker-7:main")
        .unwrap();
    assert_eq!(
        main.resolution,
        studio_engine::SceneSourceResolution::Ambiguous
    );
    assert_eq!(main.candidate_count, 2);
    assert_eq!(main.candidates.len(), 2);
    assert!(main.candidates.iter().all(|candidate| {
        candidate.symbol == "main"
            && candidate.confidence == studio_engine::SourceMatchConfidence::ExactIdentifier
    }));
    let intro = scope
        .scene_sources
        .iter()
        .find(|reference| reference.instance_id == "worker-7:intro")
        .unwrap();
    assert_eq!(
        intro.resolution,
        studio_engine::SceneSourceResolution::Missing
    );
    assert!(
        scope
            .prompt_context()
            .contains("no exact Rust identifier match")
    );

    fs::write(
        temp.path().join("src/a.rs"),
        "pub fn main() { changed(); }\n",
    )
    .unwrap();
    let mut stale = TaskScope::from_timeline(&report, &selected).unwrap();
    assert_eq!(
        stale.resolve_scene_sources(temp.path(), &inventory.files),
        Err(TaskScopeError::SourceChanged)
    );
}
