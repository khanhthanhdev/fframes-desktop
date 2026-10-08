//! On platforms without qualified no-clobber publication, a validated candidate remains
//! reviewable and exportable while Apply stays blocked.
#![cfg(not(target_os = "linux"))]
#![allow(unused_imports)]
include!("support/workflow_world.rs");

fn run_studio_tools(capability: &std::path::Path, tool: &str) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_studio-tools"))
        .arg("--capability")
        .arg(capability)
        .arg(tool)
        .output()
        .unwrap()
}

#[test]
fn an_unqualified_platform_retains_the_candidate_and_blocks_apply() {
    let w = World::new();
    let original = w.source("src/lib.rs");
    w.submit(&good("apply remains blocked"));

    let task = w.wait_phase(TaskPhase::AwaitingReview);
    let review = task.review.as_ref().unwrap();
    assert!(
        review
            .apply_blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("only proven on Linux")),
        "{review:?}"
    );
    assert_eq!(w.source("src/lib.rs"), original);
    assert_eq!(w.engine_state(), Some(TaskState::CandidateReady));

    let export = w._temp.path().join("unqualified-platform-export");
    w.wf().export_candidate(&export).unwrap();
    w.wait("the candidate export", |snapshot| {
        snapshot.rows.iter().any(|row| {
            matches!(&row.kind, RowKind::Notice { text, .. } if text.starts_with("Exported the candidate"))
        })
    });
    w.assert_clean();
}

#[test]
fn cli_project_tools_remain_available_without_the_mcp_route() {
    for (options, why) in [
        (
            Options {
                mcp: studio_agent_spike::McpStdioSupport::Unsupported,
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
                .as_deref()
                .is_some_and(|note| note.contains(why)),
            "{:?}",
            snapshot.mcp
        );
        assert_eq!(snapshot.resources.broker_grants, 1);
        let capability = snapshot.mcp.capability_file.unwrap();
        assert!(capability.is_file());
        let output = run_studio_tools(&capability, "project_context");
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
