//! The tool contract: listing, request parsing, frame selection and the dispatcher's bounds.

use super::*;

#[test]
fn all_unique_tools_are_listed_by_mcp_and_descriptions() {
    let names: HashSet<&str> = TOOL_NAMES.iter().copied().collect();
    assert_eq!(TOOL_NAMES.len(), 10);
    assert_eq!(names.len(), 10, "tool names must be unique");
    let described: Vec<String> = tool_descriptions()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(described, TOOL_NAMES);
    for tool in tool_descriptions() {
        let schema = tool["inputSchema"].as_object().unwrap();
        assert!(
            ["oneOf", "anyOf", "allOf"]
                .iter()
                .all(|operator| !schema.contains_key(*operator)),
            "{} must not expose unsupported top-level schema combinators",
            tool["name"]
        );
    }

    let mut server = McpServer::new(|| Err(ToolError::new(ToolErrorCode::Unavailable, "none")));
    initialize(&mut server);
    let list = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let listed: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, TOOL_NAMES);
    assert_eq!(list["result"]["tools"], Value::Array(tool_descriptions()));
}

#[test]
fn request_parsing_rejects_bad_shapes_and_accepts_documented_ones() {
    let code = |method: &str, params: Value| ToolRequest::parse(method, &params).unwrap_err().code;
    assert_eq!(code("shell", json!({})), ToolErrorCode::MethodNotFound);
    assert_eq!(code("export", json!({})), ToolErrorCode::MethodNotFound);
    assert_eq!(
        code("timeline", json!({"bogus": 1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "x": 1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(code("timeline", json!([1])), ToolErrorCode::InvalidParams);
    assert_eq!(
        code("timeline", json!("text")),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 2_000_000_000u64})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": -1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": 1.5})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": 0})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": -0.1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": "0.5"})),
        ToolErrorCode::InvalidParams
    );
    // NaN is not representable in JSON: it is a parse error on the wire, never a value.
    assert!(serde_json::from_str::<Value>(r#"{"frame":1,"scale":NaN}"#).is_err());
    let strip =
        |start: u64, end: u64, count: u64| json!({"start": start, "end": end, "count": count});
    assert_eq!(
        code("render_strip", strip(10, 5, 3)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_strip", strip(0, 10, 0)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_strip", strip(0, 10, MAX_STRIP_FRAMES as u64 + 1)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_strip", strip(0, 2_000_000_000, 3)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(code("inspect", json!({})), ToolErrorCode::InvalidParams);
    assert_eq!(
        code("inspect", json!({"frames": []})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "inspect",
            json!({"frames": [1], "start": 0, "end": 1, "count": 2})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("inspect", json!({"start": 4, "end": 1, "count": 2})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("inspect", json!({"start": 0, "end": 1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "inspect",
            json!({"frames": (0..1000).collect::<Vec<u64>>()})
        ),
        ToolErrorCode::InvalidParams
    );

    let ok = |method: &str, params: Value| ToolRequest::parse(method, &params).unwrap();
    assert_eq!(
        ok("project_context", Value::Null).call,
        ToolCall::ProjectContext
    );
    assert_eq!(ok("timeline", json!({})).call, ToolCall::Timeline);
    let asserted = ok(
        "build_status",
        json!({"project": "proj-1", "revision": REV_A}),
    );
    assert_eq!(asserted.call, ToolCall::BuildStatus);
    assert_eq!(asserted.assertions.project.as_deref(), Some("proj-1"));
    assert_eq!(asserted.assertions.revision.as_deref(), Some(REV_A));
    assert_eq!(
        ok("render_frame", json!({"frame": 7})).call,
        ToolCall::RenderFrame {
            frame: 7,
            scale: 0.5
        }
    );
    assert_eq!(
        ok("render_frame", json!({"frame": 7, "scale": 1.0})).call,
        ToolCall::RenderFrame {
            frame: 7,
            scale: 1.0
        }
    );
    assert_eq!(
        ok("render_strip", strip(0, 90, MAX_STRIP_FRAMES as u64)).call,
        ToolCall::RenderStrip {
            start: 0,
            end: 90,
            count: MAX_STRIP_FRAMES,
            scale: 0.5
        }
    );
    assert_eq!(
        ok("inspect", json!({"frames": [3, 1]})).call,
        ToolCall::Inspect(FrameSelection::List(vec![3, 1]))
    );
    assert_eq!(
        ok("inspect", json!({"start": 0, "end": 9, "count": 4})).call,
        ToolCall::Inspect(FrameSelection::Range {
            start: 0,
            end: 9,
            count: 4
        })
    );
    assert_eq!(
        ok(
            "selection_context",
            json!({"frame": 12, "seek_serial": 4, "frame_geometry_digest": REV_A})
        )
        .call,
        ToolCall::SelectionContext {
            frame: 12,
            seek_serial: 4,
            identity: None,
            frame_geometry_digest: Some(REV_A.into()),
        }
    );
    assert_eq!(
        ok("selection_context", json!({"frame": 12, "seek_serial": 4, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary"})).call,
        ToolCall::SelectionContext {
            frame: 12,
            seek_serial: 4,
            identity: Some(fframes_studio_protocol::EditorObjectIdentity {
                scene_instance_key: "scene-a".into(),
                component_key: "title".into(),
                object_key: "headline".into(),
                repeat_key: "primary".into(),
            }),
            frame_geometry_digest: None,
        }
    );
    assert_eq!(
        code(
            "selection_context",
            json!({"frame": 1, "seek_serial": 0, "object_key": "title"})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        ok(
            "source_lookup",
            json!({"path": "src/main.rs", "symbol": "main", "marker": "TITLE"})
        )
        .call,
        ToolCall::SourceLookup {
            path: Some(studio_project::ProjectPath::try_from("src/main.rs".to_string()).unwrap()),
            symbol: Some("main".into()),
            marker: Some("TITLE".into()),
            frame: None,
            seek_serial: None,
            identity: None,
            frame_geometry_digest: None,
        }
    );
    assert_eq!(
        ok("source_lookup", json!({"frame": 12, "seek_serial": 44, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary", "frame_geometry_digest": REV_A})).call,
        ToolCall::SourceLookup {
            path: None,
            symbol: None,
            marker: None,
            frame: Some(12),
            seek_serial: Some(44),
            identity: Some(fframes_studio_protocol::EditorObjectIdentity {
                scene_instance_key: "scene-a".into(),
                component_key: "title".into(),
                object_key: "headline".into(),
                repeat_key: "primary".into(),
            }),
            frame_geometry_digest: Some(REV_A.into()),
        }
    );
    assert_eq!(
        code("source_lookup", json!({"path": "src/main.rs"})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "source_lookup",
            json!({"path": "src/main.rs", "symbol": "main", "frame": 12, "seek_serial": 4, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary"})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "source_lookup",
            json!({"path": "../secret.rs", "symbol": "main"})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        ok("style_context", json!({"token": "color.accent"})).call,
        ToolCall::StyleContext {
            token: Some("color.accent".into()),
            frame: None,
            seek_serial: None,
            identity: None,
            frame_geometry_digest: None,
        }
    );
    assert_eq!(
        ok("style_context", json!({"frame": 12, "seek_serial": 44, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary", "frame_geometry_digest": REV_A})).call,
        ToolCall::StyleContext {
            token: None,
            frame: Some(12),
            seek_serial: Some(44),
            identity: Some(fframes_studio_protocol::EditorObjectIdentity {
                scene_instance_key: "scene-a".into(),
                component_key: "title".into(),
                object_key: "headline".into(),
                repeat_key: "primary".into(),
            }),
            frame_geometry_digest: Some(REV_A.into()),
        }
    );
    assert_eq!(
        code("style_context", json!({"frame": 12, "seek_serial": 44})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        ok("captions", json!({})).call,
        ToolCall::Captions {
            path: None,
            offset: 0,
            limit: 50,
        }
    );
    assert_eq!(
        ok(
            "captions",
            json!({"path": "media/speech.json", "offset": 50, "limit": 200})
        )
        .call,
        ToolCall::Captions {
            path: Some(
                studio_project::ProjectPath::try_from("media/speech.json".to_owned()).unwrap()
            ),
            offset: 50,
            limit: 200,
        }
    );
    for params in [
        json!({"offset": 10}),
        json!({"path": "media/speech.json", "limit": 0}),
        json!({"path": "media/speech.json", "limit": 201}),
        json!({"path": "../speech.json"}),
    ] {
        assert_eq!(code("captions", params), ToolErrorCode::InvalidParams);
    }
}

#[test]
fn frame_selection_resolve_is_bounded_sorted_and_deduplicated() {
    let list = FrameSelection::List(vec![5, 1, 5, 3]);
    assert_eq!(list.resolve(10).unwrap(), vec![1, 3, 5]);
    assert_eq!(
        list.resolve(5).unwrap_err().code,
        ToolErrorCode::InvalidParams
    );
    let range = FrameSelection::Range {
        start: 0,
        end: 9,
        count: 4,
    };
    assert_eq!(range.resolve(10).unwrap(), vec![0, 3, 6, 9]);
    assert_eq!(
        range.resolve(9).unwrap_err().code,
        ToolErrorCode::InvalidParams
    );
    let single = FrameSelection::Range {
        start: 4,
        end: 8,
        count: 1,
    };
    assert_eq!(single.resolve(5).unwrap(), vec![4]);
    let collapsed = FrameSelection::Range {
        start: 2,
        end: 3,
        count: 6,
    };
    assert_eq!(collapsed.resolve(4).unwrap(), vec![2, 3]);
}

#[test]
fn dispatcher_refuses_cross_project_and_stale_revision() {
    let backend = FakeBackend::new();
    let dispatcher = ToolDispatcher::new(backend.clone());
    let task = task();
    let fixed = ToolBinding {
        task: task.clone(),
        revision: BoundRevision::Fixed(SourceRevision::try_from(REV_A.to_string()).unwrap()),
    };
    let err = dispatcher
        .dispatch(
            &fixed,
            "timeline",
            &json!({"project": "other-project"}),
            &never,
        )
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::CrossProject);
    let err = dispatcher
        .dispatch(&fixed, "timeline", &json!({"revision": REV_B}), &never)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::StaleRevision);
    assert_eq!(
        backend.call_count(),
        0,
        "refused calls never reach the backend"
    );

    // Matching assertions pass through to the backend.
    let ok = dispatcher
        .dispatch(
            &fixed,
            "timeline",
            &json!({"project": "proj-1", "revision": REV_A}),
            &never,
        )
        .unwrap();
    assert_eq!(ok["revision"]["id"], REV_A);
    assert_eq!(backend.call_count(), 1);

    // A draft binding cannot be asserted against a revision the reply is not about.
    let err = dispatcher
        .dispatch(
            &draft(&task),
            "timeline",
            &json!({"revision": REV_B}),
            &never,
        )
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::StaleRevision);

    // A call cancelled before it starts never runs.
    let before = backend.call_count();
    let err = dispatcher
        .dispatch(&draft(&task), "timeline", &json!({}), &|| true)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::Cancelled);
    assert_eq!(backend.call_count(), before);
}

#[test]
fn dispatcher_bounds_text_replies_and_artifacts() {
    let backend = FakeBackend::new();
    let dispatcher = ToolDispatcher::new(backend.clone());
    let binding = draft(&task());

    backend.set(|_, request, _| {
        Ok(ToolReply {
            result: json!({"blob": "x".repeat(MAX_TEXT_REPLY_BYTES + 1)}),
            ..plain_reply(request)
        })
    });
    let err = dispatcher
        .dispatch(&binding, "timeline", &json!({}), &never)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::TooLarge);

    backend.set(|_, request, _| {
        Ok(ToolReply {
            result: json!({"blob": "x".repeat(MAX_TEXT_REPLY_BYTES / 2)}),
            ..plain_reply(request)
        })
    });
    let value = dispatcher
        .dispatch(&binding, "timeline", &json!({}), &never)
        .unwrap();
    assert!(serde_json::to_vec(&value).unwrap().len() <= MAX_TEXT_REPLY_BYTES);

    backend.set(|_, request, _| {
        Ok(ToolReply {
            artifacts: vec![artifact(MAX_IMAGE_ARTIFACT_BYTES as u64 + 1)],
            ..plain_reply(request)
        })
    });
    let err = dispatcher
        .dispatch(&binding, "render_frame", &json!({"frame": 1}), &never)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::TooLarge);

    backend.set(|_, request, _| {
        Ok(ToolReply {
            artifacts: vec![artifact(MAX_IMAGE_ARTIFACT_BYTES as u64)],
            ..plain_reply(request)
        })
    });
    let value = dispatcher
        .dispatch(&binding, "render_frame", &json!({"frame": 1}), &never)
        .unwrap();
    assert_eq!(
        value["artifacts"][0]["bytes"],
        MAX_IMAGE_ARTIFACT_BYTES as u64
    );
}

fn fake_identity(n: usize) -> String {
    format!("{n:064x}")
}

#[test]
fn known_revision_history_is_bounded_lru_and_status_summary_stays_small() {
    let mut history = RevisionHistory::default();
    assert!(history.is_empty());
    // Far past the size the old unbounded list would have reached: 5000 x 64-hex ids
    // serialise to well over the 256 KiB reply limit.
    let captured = 5000;
    let unbounded = serde_json::to_vec(&(0..captured).map(fake_identity).collect::<Vec<_>>())
        .unwrap()
        .len();
    assert!(unbounded > MAX_TEXT_REPLY_BYTES, "{unbounded}");
    for n in 0..captured {
        history.remember(&fake_identity(n));
        assert!(history.len() <= MAX_KNOWN_REVISIONS);
    }
    assert_eq!(history.len(), MAX_KNOWN_REVISIONS);
    assert_eq!(history.evicted(), (captured - MAX_KNOWN_REVISIONS) as u64);
    // Oldest are evicted first: the newest MAX_KNOWN_REVISIONS stay, everything older is gone.
    assert!(!history.contains(&fake_identity(0)));
    assert!(!history.contains(&fake_identity(captured - MAX_KNOWN_REVISIONS - 1)));
    assert!(history.contains(&fake_identity(captured - MAX_KNOWN_REVISIONS)));
    assert!(history.contains(&fake_identity(captured - 1)));

    let summary = history.summary(STATUS_KNOWN_REVISIONS);
    assert_eq!(summary.recent.len(), STATUS_KNOWN_REVISIONS);
    assert_eq!(summary.total, captured as u64);
    assert_eq!(
        summary.recent.last().unwrap(),
        &fake_identity(captured - 1),
        "newest last"
    );
    let listed = serde_json::to_vec(&summary.recent).unwrap().len();
    assert!(listed < 2 * 1024, "{listed}");
    const {
        assert!(
            (MAX_KNOWN_REVISIONS * 70) < MAX_TEXT_REPLY_BYTES / 32,
            "even the whole retained set is a small fraction of the reply limit"
        )
    };

    // Using a retained identity refreshes it, so it outlives newer-but-unused ones.
    let oldest_kept = fake_identity(captured - MAX_KNOWN_REVISIONS);
    history.remember(&oldest_kept);
    for n in captured..captured + MAX_KNOWN_REVISIONS - 1 {
        history.remember(&fake_identity(n));
    }
    assert!(history.contains(&oldest_kept));
    assert!(!history.contains(&fake_identity(captured - MAX_KNOWN_REVISIONS + 1)));
    history.remember(&fake_identity(captured + MAX_KNOWN_REVISIONS));
    assert!(!history.contains(&oldest_kept), "now it is the oldest");

    // A short history reports everything and no truncation.
    let mut small = RevisionHistory::default();
    small.remember(REV_A);
    small.remember(REV_B);
    small.remember(REV_A);
    let summary = small.summary(STATUS_KNOWN_REVISIONS);
    assert_eq!(summary.recent, vec![REV_B.to_owned(), REV_A.to_owned()]);
    assert_eq!(summary.total, 2);
}
