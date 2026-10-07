use fframes_studio::agent_tools::{
    BoundRevision, RevisionInfo, RevisionLabel, ToolBackend, ToolBinding, ToolCall, ToolDispatcher,
    ToolError, ToolReply, ToolRequest,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use studio_engine::{AgentTaskId, OpenSession, TaskIdentity};
use studio_project::{ProjectId, SourceRevision};

const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_REVISION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Default)]
struct RecordingBackend(Mutex<Vec<String>>);

impl ToolBackend for RecordingBackend {
    fn execute(
        &self,
        _: &ToolBinding,
        request: &ToolRequest,
        _: &(dyn Fn() -> bool + Sync),
    ) -> Result<ToolReply, ToolError> {
        let name = request.call.name().to_owned();
        self.0.lock().push(name.clone());
        Ok(ToolReply {
            revision: RevisionInfo {
                id: REVISION.into(),
                label: RevisionLabel::TaskBase,
                validated: false,
            },
            result: json!({"tool": name}),
            artifacts: Vec::new(),
        })
    }
}

fn binding() -> ToolBinding {
    ToolBinding {
        task: TaskIdentity {
            task: AgentTaskId::new(),
            project: ProjectId::try_from("project-1".to_owned()).unwrap(),
            session: OpenSession::new(),
            generation: 1,
        },
        revision: BoundRevision::Fixed(SourceRevision::try_from(REVISION.to_owned()).unwrap()),
    }
}

fn never() -> bool {
    false
}

fn selected(frame_geometry_digest: &str) -> Value {
    json!({
        "frame": 12,
        "seek_serial": 44,
        "scene_instance_key": "scene-a",
        "component_key": "title",
        "object_key": "headline",
        "repeat_key": "primary",
        "frame_geometry_digest": frame_geometry_digest,
    })
}

#[test]
fn selection_retrieval_tools_share_dispatch_and_revision_fences() {
    let backend = Arc::new(RecordingBackend::default());
    let dispatcher = ToolDispatcher::new(backend.clone());
    let binding = binding();
    let geometry_digest = "c".repeat(64);
    let calls = [
        (
            "selection_context",
            json!({"frame": 12, "seek_serial": 44, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary", "frame_geometry_digest": geometry_digest.clone()}),
        ),
        (
            "source_lookup",
            json!({"path": "src/lib.rs", "symbol": "render_frame", "marker": "title-source-marker"}),
        ),
        ("source_lookup", selected(&geometry_digest)),
        ("style_context", json!({"token": "typography.title"})),
        ("style_context", selected(&geometry_digest)),
    ];

    let call_count = calls.len();
    for (name, params) in calls {
        let result = dispatcher
            .dispatch(&binding, name, &params, &never)
            .unwrap();
        assert_eq!(result["revision"]["id"], REVISION, "{name}: {params}");
        assert_eq!(result["result"]["tool"], name, "{name}: {params}");
    }
    assert_eq!(
        *backend.0.lock(),
        [
            "selection_context",
            "source_lookup",
            "source_lookup",
            "style_context",
            "style_context"
        ]
    );

    for (name, params) in [
        (
            "selection_context",
            json!({"revision": OTHER_REVISION, "frame": 12, "seek_serial": 44}),
        ),
        (
            "source_lookup",
            json!({"revision": OTHER_REVISION, "path": "src/lib.rs", "symbol": "render_frame"}),
        ),
        (
            "style_context",
            json!({"revision": OTHER_REVISION, "token": "typography.title"}),
        ),
    ] {
        let error = dispatcher
            .dispatch(&binding, name, &params, &never)
            .unwrap_err();
        assert_eq!(
            error.code,
            fframes_studio::agent_tools::ToolErrorCode::StaleRevision
        );
    }
    assert_eq!(
        backend.0.lock().len(),
        call_count,
        "stale calls never execute"
    );
}

#[test]
fn selection_retrieval_tools_reject_partial_or_ambiguous_selection_queries() {
    for (name, params) in [
        ("selection_context", json!({"frame": 1})),
        (
            "selection_context",
            json!({"frame": 1, "seek_serial": 2, "object_key": "title"}),
        ),
        (
            "source_lookup",
            json!({"path": "src/lib.rs", "marker": "title-source-marker"}),
        ),
        (
            "source_lookup",
            json!({"frame": 1, "seek_serial": 2, "scene_instance_key": "scene"}),
        ),
        ("style_context", json!({"frame": 1, "seek_serial": 2})),
        (
            "style_context",
            json!({"token": "typography.title", "unexpected": true}),
        ),
    ] {
        assert!(
            ToolRequest::parse(name, &params).is_err(),
            "accepted malformed {name} params: {params}"
        );
    }

    assert!(matches!(
        ToolRequest::parse(
            "source_lookup",
            &json!({"path": "src/lib.rs", "symbol": "render_frame"})
        )
        .unwrap()
        .call,
        ToolCall::SourceLookup { .. }
    ));
    assert!(matches!(
        ToolRequest::parse("source_lookup", &selected(&"d".repeat(64)))
            .unwrap()
            .call,
        ToolCall::SourceLookup { .. }
    ));
    assert!(matches!(
        ToolRequest::parse("style_context", &selected(&"e".repeat(64)))
            .unwrap()
            .call,
        ToolCall::StyleContext { .. }
    ));
}
