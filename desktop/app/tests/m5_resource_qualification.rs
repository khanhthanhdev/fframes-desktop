use std::{fs, time::Instant};

use fframes_studio_protocol::{
    EditorFrameMetadata, EditorFrameStatus, EditorGeometrySupport, EditorObjectGeometry,
    EditorObjectIdentity, MAX_EDITOR_METADATA_BYTES, MAX_EDITOR_OBJECTS_PER_FRAME, PreviewIdentity,
    Rect,
};
use sha2::{Digest, Sha256};
use studio_engine::{CanvasRect, CanvasViewport, DisplayedFrameTag, hit_test};
use studio_project::{
    ProjectPath, SourceRevision,
    revision::FileKind,
    source_index::{SourceAnchor, SourceIndex, SourceIndexInput},
};

fn percentile(values: &mut [u128], numerator: usize, denominator: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * numerator / denominator]
}

#[test]
fn m5_resource_measurements_are_bounded() {
    let target_source = "pub fn render_title() { /* m5-resource-anchor */ }\n";
    let source_inputs: Vec<_> = (0..256)
        .map(|index| {
            let text = if index == 0 {
                target_source.to_owned()
            } else {
                format!("pub fn helper_{index}() {{}}\n")
            };
            let bytes = text.into_bytes();
            SourceIndexInput {
                path: ProjectPath::try_from(format!("src/module_{index:03}.rs")).unwrap(),
                kind: FileKind::Rust,
                expected_sha256: format!("{:x}", Sha256::digest(&bytes)),
                expected_size: bytes.len() as u64,
                bytes,
            }
        })
        .collect();
    let source_bytes: usize = source_inputs.iter().map(|input| input.bytes.len()).sum();
    let revision: SourceRevision = "f".repeat(64).try_into().unwrap();
    let started = Instant::now();
    let index = SourceIndex::build(revision, source_inputs, &|| false).unwrap();
    let index_build_micros = started.elapsed().as_micros();
    assert_eq!(index.indexed_files, 256);
    assert_eq!(index.indexed_bytes, source_bytes);

    let anchor = SourceAnchor {
        path: ProjectPath::try_from("src/module_000.rs".to_owned()).unwrap(),
        symbol: "render_title".into(),
        expected_sha256: format!("{:x}", Sha256::digest(target_source.as_bytes())),
        marker: Some("m5-resource-anchor".into()),
    };
    let mut lookup_samples = Vec::with_capacity(50);
    for _ in 0..50 {
        let started = Instant::now();
        let result = index.lookup(&anchor).unwrap();
        lookup_samples.push(started.elapsed().as_micros());
        assert_eq!(result.snippets[0].confidence, "explicit_marker");
    }

    let width = 1920;
    let height = 1080;
    let metadata = EditorFrameMetadata {
        frame_index: 0,
        seek_serial: 1,
        video_width: width,
        video_height: height,
        editor_index_digest: "a".repeat(64),
        frame_geometry_digest: "b".repeat(64),
        status: EditorFrameStatus::Supported,
        reason: None,
        objects: (0..MAX_EDITOR_OBJECTS_PER_FRAME)
            .map(|index| EditorObjectGeometry {
                identity: EditorObjectIdentity {
                    scene_instance_key: "s".into(),
                    component_key: "c".into(),
                    object_key: format!("o{index:04}"),
                    repeat_key: "r".into(),
                },
                parent: None,
                source_anchor: None,
                style_tokens: Vec::new(),
                bounds: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: width as f32,
                    height: height as f32,
                },
                paint_order: index as u32,
                support: EditorGeometrySupport::ExactBounds,
            })
            .collect(),
    };
    let object_limit_metadata_bytes = serde_json::to_vec(&metadata).unwrap().len();
    let metadata_budget = MAX_EDITOR_METADATA_BYTES - 4096;
    let metadata_bytes = object_limit_metadata_bytes;
    assert!(metadata_bytes <= metadata_budget);
    let tag = DisplayedFrameTag::from_metadata(
        PreviewIdentity {
            project_id: "m5-resource-fixture".into(),
            open_session: "qualification-session".into(),
            source_revision: "c".repeat(64),
            worker_generation: 1,
        },
        &metadata,
    )
    .unwrap();
    let viewport = CanvasViewport::new(
        CanvasRect {
            x: 0.0,
            y: 0.0,
            width: 960.0,
            height: 540.0,
        },
        width,
        height,
    )
    .unwrap();
    let mut hit_test_samples = Vec::with_capacity(30);
    for _ in 0..30 {
        let started = Instant::now();
        let selected = hit_test(&metadata, &tag, &viewport, 480.0, 270.0)
            .unwrap()
            .unwrap();
        hit_test_samples.push(started.elapsed().as_micros());
        assert_eq!(
            selected.identity.object_key,
            format!("o{:04}", metadata.objects.len() - 1)
        );
    }

    let mut lookup_p95 = lookup_samples.clone();
    let mut hit_test_p95 = hit_test_samples.clone();
    let measurements = serde_json::json!({
        "schema": "m5-resource-measurements/1",
        "fixture_only": true,
        "source": {
            "file_count": index.indexed_files,
            "indexed_bytes": index.indexed_bytes,
            "build_micros": index_build_micros,
            "lookup_samples": lookup_samples.len(),
            "lookup_p95_micros": percentile(&mut lookup_p95, 95, 100),
        },
        "frame": {
            "width": width,
            "height": height,
            "object_count": metadata.objects.len(),
            "serialized_metadata_bytes": metadata_bytes,
            "serialized_bytes_at_object_limit": object_limit_metadata_bytes,
            "objects_excluded_by_metadata_byte_limit": 0,
            "metadata_budget_bytes": metadata_budget,
            "worker_overflow_policy": "reject_without_partial_objects",
            "hit_test_samples": hit_test_samples.len(),
            "hit_test_p95_micros": percentile(&mut hit_test_p95, 95, 100),
        },
        "limits": {
            "source_files": studio_project::source_index::MAX_INDEX_RUST_FILES,
            "source_bytes": studio_project::source_index::MAX_INDEX_TOTAL_BYTES,
            "frame_objects": MAX_EDITOR_OBJECTS_PER_FRAME,
        },
    });
    let encoded = serde_json::to_vec_pretty(&measurements).unwrap();
    if let Some(path) = std::env::var_os("M5_RESOURCE_EVIDENCE") {
        fs::write(path, &encoded).unwrap();
    } else {
        println!("{}", String::from_utf8(encoded).unwrap());
    }
}
