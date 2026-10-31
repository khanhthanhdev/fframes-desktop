//! The Apply gate on every platform: a validated candidate is published where the
//! project's filesystem proves no-clobber publication, and stays reviewable and
//! exportable, with nothing written, where the probe refuses it.
#![allow(dead_code, unused_imports)]
include!("../../crates/studio-engine/tests/support/workflow_world.rs");

fn run_studio_tools(capability: &std::path::Path, tool: &str) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_studio-tools"))
        .arg("--capability")
        .arg(capability)
        .arg(tool)
        .output()
        .unwrap()
}

#[test]
fn a_validated_candidate_is_applied_where_publication_is_proven() {
    let w = World::new();
    assert!(w.ctl().lock().apply_gate().is_ready());
    w.submit(&good("applied on this platform"));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(
        w.source("src/lib.rs"),
        "// applied on this platform
"
    );
    w.assert_clean();
}

#[test]
fn a_refused_probe_retains_the_candidate_and_blocks_apply() {
    let w = World::new();
    // A filesystem that offers only the link-then-unlink pair fails the real probe.
    w.ctl()
        .lock()
        .force_publication_mechanism(Some(studio_engine::edit_transaction::NoClobber::Link));
    let original = w.source("src/lib.rs");
    w.submit(&good("apply remains blocked"));

    let task = w
        .wait("candidate review or validation failure", |snapshot| {
            snapshot.task.as_ref().is_some_and(|task| {
                matches!(task.phase, TaskPhase::AwaitingReview | TaskPhase::Failed)
            })
        })
        .task
        .clone()
        .unwrap();
    assert_eq!(
        task.phase,
        TaskPhase::AwaitingReview,
        "candidate validation did not reach review; agent prompts: {:?}",
        w.evidence("prompts.jsonl")
    );
    let review = task.review.as_ref().unwrap();
    assert!(
        review
            .apply_blocked
            .as_deref()
            .is_some_and(|reason| reason.contains(studio_engine::edit_transaction::LINK_REFUSED)),
        "{review:?}"
    );
    assert_eq!(w.source("src/lib.rs"), original);
    assert_eq!(w.engine_state(), Some(TaskState::CandidateReady));

    let export = w._temp.path().join("blocked-export");
    w.wf().export_candidate(&export).unwrap();
    w.wait("the candidate export", |snapshot| {
        snapshot.rows.iter().any(|row| {
            matches!(&row.kind, RowKind::Notice { text, .. } if text.starts_with("Exported the candidate"))
        })
    });
    w.wf().discard().unwrap();
    assert_eq!(w.wait_task(None).phase, TaskPhase::Cancelled);
    assert_eq!(w.source("src/lib.rs"), original);
    w.assert_clean();
}

#[test]
fn cli_project_tools_remain_available_without_the_mcp_route() {
    for (options, why) in [
        (
            Options {
                mcp: studio_harness::McpStdioSupport::Unsupported,
                ..Options::default()
            },
            "disabled by host policy",
        ),
        (
            Options {
                mcp_helper: false,
                ..Options::default()
            },
            "helper was not found",
        ),
    ] {
        let w = World::with(options);
        w.submit(&serde_json::json!({"hang": true}));
        w.wait_phase(TaskPhase::Editing);
        w.wait_prompts(1);
        let params: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(w.agent.join("session-params.json")).unwrap(),
        )
        .unwrap();
        assert!(params["mcpServers"].as_array().unwrap().is_empty());

        let snapshot = w.wait("the CLI-only tool route", |snapshot| {
            !snapshot.mcp.active && snapshot.mcp.cli_active
        });
        assert!(
            snapshot
                .mcp
                .note
                .as_ref()
                .is_some_and(|note| note.to_string().contains(why)),
            "{:?}",
            snapshot.mcp
        );
        assert_eq!(snapshot.resources.broker_grants, 1);
        let capability = snapshot.mcp.capability_file.as_ref().unwrap();
        assert!(capability.is_file());
        let output = run_studio_tools(capability, "project_context");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reply: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(reply.is_object(), "{reply}");
        let prompt = w.evidence("prompts.jsonl")[0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(prompt.contains("studio-tools") && prompt.contains(&*capability.to_string_lossy()));
        w.wf().stop().unwrap();
        w.wait_task(None);
        w.assert_clean();
    }
}

#[test]
fn the_user_s_servers_join_the_session_and_turned_off_tools_are_not_offered() {
    use studio_engine::agent_settings::{AgentToolSettings, UserMcpServer};
    let w = World::new();
    let server = |name: &str, env: &[&str]| {
        UserMcpServer::stdio(
            name,
            std::env::current_exe().unwrap(),
            vec!["--serve".into()],
            env.iter().map(|name| (*name).to_owned()).collect(),
        )
    };
    w.wf()
        .set_agent_tools(AgentToolSettings {
            mcp_servers: vec![
                server("docs", &[]),
                server("needs-token", &["FFRAMES_STUDIO_TEST_NEVER_SET"]),
                // This ACP agent does not advertise HTTP servers: it is left out.
                UserMcpServer::http(
                    "remote",
                    "https://remote.test/mcp",
                    None,
                    Default::default(),
                ),
            ],
            disabled_tools: vec!["render_strip".into()],
        })
        .unwrap();
    w.submit(&serde_json::json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    let params: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(w.agent.join("session-params.json")).unwrap(),
    )
    .unwrap();
    let names: Vec<&str> = params["mcpServers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|server| server["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fframes-studio", "docs"], "{params}");

    // Once the session started, the note says exactly which servers it did not get.
    let snapshot = w.wait("the MCP route and the session's servers", |snapshot| {
        snapshot.mcp.active
            && snapshot
                .mcp
                .note
                .as_ref()
                .is_some_and(|notes| notes.0.len() == 2)
    });
    let notes = snapshot.mcp.note.clone().unwrap();
    let note = notes.to_string();
    assert!(
        note.contains("`needs-token` was left out: FFRAMES_STUDIO_TEST_NEVER_SET is not set"),
        "{note}"
    );
    assert!(
        note.contains("`remote` was left out: this agent does not support HTTP MCP servers."),
        "{note}"
    );
    assert!(!note.contains("only if"), "the note is exact: {note}");
    use studio_engine::agent::model::ToolNoteKind;
    let left_out: Vec<_> = notes.0.iter().map(|note| note.kind.clone()).collect();
    assert_eq!(
        left_out,
        ["needs-token", "remote"].map(|name| ToolNoteKind::ServerLeftOut { name: name.into() })
    );
    let capability: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(snapshot.mcp.capability_file.as_ref().unwrap()).unwrap(),
    )
    .unwrap();
    let granted: Vec<&str> = capability["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool.as_str().unwrap())
        .collect();
    assert!(!granted.contains(&"render_strip") && granted.contains(&"render_frame"));
    let prompt = w.evidence("prompts.jsonl")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(prompt.contains("render_frame") && !prompt.contains("render_strip"));
    w.wf().stop().unwrap();
    w.wait_task(None);
    w.assert_clean();
}
