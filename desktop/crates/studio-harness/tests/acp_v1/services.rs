//! Redaction, declined services, session restore, options and process ownership.

use super::*;

// ---- redaction, declined services, restore, options ----------------------------------------------------

#[test]
fn configured_secret_values_never_reach_events_transcripts_stderr_or_failures() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "secret");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    wait_for(|| (!driver.stderr_tail().is_empty()).then_some(()));
    let events = driver.drain_events(256);
    let dump = format!(
        "{events:?} {:?} {}",
        driver.transcript_page(0, 100),
        driver.stderr_tail()
    );
    assert!(!dump.contains(SECRET), "secret leaked: {dump}");
    assert!(dump.contains("[REDACTED]"));
    let joined: String = driver
        .transcript_page(0, 100)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert!(joined.contains("The key is") && joined.contains("done"));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "auth-rejected");
    cfg.launch = cfg.launch.with_auth_method("key");
    let (driver, _manager) = start_with(cfg);
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::AuthRejected);
    assert!(!format!("{failure:?}").contains(SECRET));
    drop(driver.shutdown());
}

#[test]
fn optional_filesystem_and_terminal_services_are_declined() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "fs-request");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    for name in ["fs-1.json", "term-1.json"] {
        let response = read_json(root.path(), name);
        assert_eq!(response["error"]["code"], -32601, "{name}: {response}");
    }
    assert_eq!(driver.declined_requests(), 2);
    assert!(driver.failure().is_none());
    drop(driver.shutdown());
}

#[test]
fn restoration_requires_a_negotiated_capability() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "load");
    cfg.resume_session = Some("saved-session".into());
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.restored);
    assert_eq!(info.session_id.as_deref(), Some("saved-session"));
    assert_eq!(
        read_json(root.path(), "session-params.json")["method"],
        "session/load"
    );
    assert!(
        driver.transcript_page(0, 10).entries.is_empty(),
        "replayed history is not a row source"
    );
    let events = driver.drain_events(256);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, AgentEventKind::MessageDelta { .. })),
        "replay produces no live message events: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            AgentEventKind::AvailableCommands(commands) if commands[0].name == "compact"
        )),
        "commands survive a restore: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            &e.kind,
            AgentEventKind::Plan(entries) if entries[0].text == "resume work"
        )),
        "the latest plan survives a restore: {events:?}"
    );
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    let live: String = driver
        .transcript_page(0, 10)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert_eq!(live, "Hello world", "only live text follows a restore");
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.resume_session = Some("saved-session".into());
    let (driver, _manager) = start_with(cfg);
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::RestoreUnsupported);
    let calls = read_json(root.path(), "calls.json");
    assert!(
        !calls.as_array().unwrap().iter().any(|m| m == "session/new"),
        "no silent fresh session in place of a restore: {calls}"
    );
    drop(driver.shutdown());
}

#[test]
fn only_advertised_options_can_be_set_and_adapter_rejections_are_events() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "options");
    ready(&driver);
    let options = driver.options();
    assert_eq!(options.current_mode.as_deref(), Some("code"));
    assert_eq!(options.modes.len(), 2);
    assert_eq!(options.config[0].id, "model");

    assert!(matches!(
        driver.set_mode("nope"),
        Err(DriverError::InvalidOption(_))
    ));
    assert!(matches!(
        driver.set_config_option("unknown", OptionValue::Value("x".into())),
        Err(DriverError::InvalidOption(_))
    ));
    assert!(matches!(
        driver.set_config_option("model", OptionValue::Value("bogus".into())),
        Err(DriverError::InvalidOption(_))
    ));
    assert!(matches!(
        driver.set_config_option("model", OptionValue::Boolean(true)),
        Err(DriverError::InvalidOption(_))
    ));

    driver.set_mode("plan").unwrap();
    collect_until(
        &driver,
        |e| matches!(&e.kind, AgentEventKind::Options(o) if o.current_mode.as_deref() == Some("plan")),
    );
    assert_eq!(read_json(root.path(), "set-mode.json")["modeId"], "plan");
    driver
        .set_config_option("model", OptionValue::Value("deep".into()))
        .unwrap();
    collect_until(&driver, |e| {
        matches!(&e.kind, AgentEventKind::Options(o)
            if matches!(&o.config[0].kind, studio_harness::driver::ConfigKind::Select { current, .. } if current == "deep"))
    });
    // Advertised but refused by the adapter: visible event, session unchanged.
    driver
        .set_config_option("model", OptionValue::Value("fast".into()))
        .unwrap();
    collect_until(
        &driver,
        |e| matches!(&e.kind, AgentEventKind::OptionRejected { option, .. } if option == "model"),
    );
    assert!(driver.failure().is_none());
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "ok");
    ready(&driver);
    assert!(matches!(
        driver.set_mode("code"),
        Err(DriverError::InvalidOption(_))
    ));
    drop(driver.shutdown());
}

// ---- process ownership -------------------------------------------------------------------------------

#[test]
#[cfg(unix)]
fn helpers_inside_the_task_group_are_reaped_and_group_cleanup_is_verified() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start_with(config(root.path(), "spawn-helper"));
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    let helper = helper_pid(root.path());
    assert!(pid_running(helper));
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified(), "{:?}", outcome.termination);
    assert!(outcome.escaped_pids.is_empty());
    assert!(outcome.ownership.is_qualified());
    assert!(!pid_running(helper), "owned helper survived group cleanup");
    assert_eq!(manager.active_count(), 0);
}

#[test]
#[cfg(unix)]
fn a_setsid_escaped_helper_demotes_ownership_and_is_not_covered_by_group_cleanup() {
    for mode in ["escape-helper", "escape-pipes"] {
        let root = tempfile::tempdir().unwrap();
        let (driver, manager) = start_with(config(root.path(), mode));
        assert!(driver.ownership().is_qualified(), "{mode}");
        ready(&driver);
        let turn = driver.prompt("go").unwrap();
        driver.wait_turn(turn, WAIT).unwrap();
        let helper = helper_pid(root.path());
        let stop = Instant::now();
        let outcome: DriverOutcome = driver.shutdown();
        let survived = pid_running(helper);
        // Clean up only the known, test-owned helper before asserting.
        kill_known_pid(helper);
        assert!(
            stop.elapsed() < Duration::from_secs(8),
            "{mode}: a helper holding the pipes must not wedge shutdown"
        );
        assert!(
            outcome.termination.verified(),
            "group cleanup is verifiable even though the helper escaped it: {mode}"
        );
        assert!(
            survived,
            "{mode}: escaped helper must outlive group cleanup"
        );
        {
            assert_eq!(outcome.escaped_pids, vec![helper as u32], "{mode}");
            assert_eq!(
                outcome.ownership,
                WriterOwnership::Detached,
                "{mode}: qualification gate must fail"
            );
            assert!(!outcome.ownership.is_qualified());
        }
        assert_eq!(manager.active_count(), 0);
    }
}

#[test]
fn dropping_the_driver_reaps_the_adapter() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "hang-prompt");
    ready(&driver);
    driver.prompt("go").unwrap();
    assert_eq!(manager.active_count(), 1);
    drop(driver);
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn spawn_rejects_a_relative_or_missing_session_directory() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.cwd = PathBuf::from("relative/dir");
    let manager = ProcessTreeManager::new();
    let failure = AcpDriver::start(cfg, &manager).err().unwrap();
    assert_eq!(failure.kind, FailureKind::SpawnFailed);
    assert_eq!(manager.active_count(), 0);
}

fn project_mcp_server(root: &Path) -> McpStdioServer {
    McpStdioServer::new(
        "fframes-project",
        root.join("bin").join("studio-mcp"),
        vec!["--task".into(), "task-1".into()],
        vec![
            ("FFRAMES_MCP_TOKEN".into(), MCP_SECRET.into()),
            ("FFRAMES_TASK".into(), "task-1".into()),
        ],
    )
    .expect("valid server")
}

fn expected_wire(root: &Path) -> serde_json::Value {
    serde_json::json!([{
        "name": "fframes-project",
        "command": root.join("bin").join("studio-mcp"),
        "args": ["--task", "task-1"],
        "env": [
            {"name": "FFRAMES_MCP_TOKEN", "value": MCP_SECRET},
            {"name": "FFRAMES_TASK", "value": "task-1"},
        ],
    }])
}

#[test]
fn configured_stdio_servers_reach_session_new_and_restore_requests_verbatim() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.initialized.capabilities.mcp_stdio);
    let params = read_json(root.path(), "session-params.json");
    assert_eq!(params["method"], "session/new");
    assert_eq!(params["mcpServers"], expected_wire(root.path()));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "load");
    cfg.resume_session = Some("saved-session".into());
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let (driver, _manager) = start_with(cfg);
    driver.wait_ready(WAIT).unwrap();
    let params = read_json(root.path(), "session-params.json");
    assert_eq!(params["method"], "session/load");
    assert_eq!(params["mcpServers"], expected_wire(root.path()));
    drop(driver.shutdown());
}

#[test]
fn user_http_servers_reach_the_session_only_when_the_agent_takes_http() {
    let user = |root: &Path| -> Vec<UserMcpLaunch> {
        let mut docs: UserMcpLaunch = McpHttpServer::new(
            "docs",
            "https://docs.test/mcp",
            vec![("Authorization".into(), "Bearer user-http-secret-88".into())],
        )
        .unwrap()
        .into();
        // ACP cannot filter a server's tools; the switch has no wire form.
        docs.disabled_tools = vec!["delete_page".into()];
        let files = McpStdioServer::new("files", root.join("bin").join("files"), vec![], vec![])
            .unwrap()
            .into();
        vec![files, docs]
    };
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "mcp-http");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    cfg.user_mcp_servers = user(root.path());
    let sent = |driver: &AcpDriver| -> Vec<String> {
        let events = collect_until(driver, |event| {
            matches!(event.kind, AgentEventKind::SessionReady { .. })
        });
        match &events.last().unwrap().kind {
            AgentEventKind::SessionReady {
                user_mcp_servers, ..
            } => user_mcp_servers.clone(),
            _ => unreachable!(),
        }
    };
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.initialized.capabilities.mcp_http);
    assert_eq!(sent(&driver), ["files", "docs"]);
    let servers = read_json(root.path(), "session-params.json")["mcpServers"].clone();
    let names: Vec<&str> = servers
        .as_array()
        .unwrap()
        .iter()
        .map(|server| server["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fframes-project", "files", "docs"]);
    assert_eq!(
        servers[2],
        serde_json::json!({
            "type": "http",
            "name": "docs",
            "url": "https://docs.test/mcp",
            "headers": [{"name": "Authorization", "value": "Bearer user-http-secret-88"}],
        })
    );
    drop(driver.shutdown());

    // An agent that did not advertise `mcpCapabilities.http` gets the stdio servers only.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    cfg.user_mcp_servers = user(root.path());
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(!info.initialized.capabilities.mcp_http);
    // The host learns exactly which of the user's servers went out.
    assert_eq!(sent(&driver), ["files"]);
    let servers = read_json(root.path(), "session-params.json")["mcpServers"].clone();
    let names: Vec<&str> = servers
        .as_array()
        .unwrap()
        .iter()
        .map(|server| server["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fframes-project", "files"]);
    assert!(!servers.to_string().contains("user-http-secret-88"));
    drop(driver.shutdown());
}

#[test]
fn unsupported_or_empty_stdio_configuration_sends_an_empty_list_and_reports_the_capability() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "ok");
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.initialized.capabilities.mcp_stdio);
    assert_eq!(
        read_json(root.path(), "session-params.json")["mcpServers"],
        serde_json::json!([])
    );
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    cfg.mcp_stdio = McpStdioSupport::Unsupported;
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(!info.initialized.capabilities.mcp_stdio);
    assert_eq!(
        read_json(root.path(), "session-params.json")["mcpServers"],
        serde_json::json!([])
    );
    let events = driver.drain_events(256);
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        AgentEventKind::Initialized(info) if !info.capabilities.mcp_stdio
    )));
    drop(driver.shutdown());
}

#[test]
fn mcp_env_values_never_reach_events_transcripts_stderr_failures_or_debug_output() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "mcp-secret");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let debug = format!("{cfg:?}");
    assert!(!debug.contains(MCP_SECRET), "config debug leaked: {debug}");
    assert!(debug.contains("FFRAMES_MCP_TOKEN"));
    let (driver, _manager) = start_with(cfg);
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    wait_for(|| (!driver.stderr_tail().is_empty()).then_some(()));
    let events = driver.drain_events(256);
    let dump = format!(
        "{events:?} {:?} {}",
        driver.transcript_page(0, 100),
        driver.stderr_tail()
    );
    assert!(!dump.contains(MCP_SECRET), "mcp secret leaked: {dump}");
    assert!(dump.contains("[REDACTED]"));
    let joined: String = driver
        .transcript_page(0, 100)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert!(joined.contains("The mcp key is") && joined.contains("done"));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "mcp-reject");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let (driver, _manager) = start_with(cfg);
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::SessionRejected);
    assert!(
        !format!("{failure:?} {failure}").contains(MCP_SECRET),
        "failure leaked: {failure:?}"
    );
    let dump = format!("{:?} {}", driver.drain_events(256), driver.stderr_tail());
    assert!(!dump.contains(MCP_SECRET), "events leaked: {dump}");
    drop(driver.shutdown());
}

#[test]
fn mcp_server_construction_rejects_relative_commands_and_unsafe_strings() {
    let abs = std::env::temp_dir().join("studio-mcp");
    let new = |name: &str, command: PathBuf, args: Vec<String>, env: Vec<(String, String)>| {
        McpStdioServer::new(name, command, args, env)
    };
    assert_eq!(
        new("srv", "relative/studio-mcp".into(), vec![], vec![]),
        Err(McpConfigError::RelativeCommand { name: "srv".into() })
    );
    assert_eq!(
        new("", abs.clone(), vec![], vec![]),
        Err(McpConfigError::EmptyName)
    );
    assert!(matches!(
        new("a\0b", abs.clone(), vec![], vec![]),
        Err(McpConfigError::Nul { .. })
    ));
    assert!(matches!(
        new("srv", abs.clone(), vec!["x\0".into()], vec![]),
        Err(McpConfigError::Nul { .. })
    ));
    assert!(matches!(
        new("srv", abs.clone(), vec![], vec![("K".into(), "v\0".into())]),
        Err(McpConfigError::Nul { .. })
    ));
    assert!(matches!(
        new("srv", abs.clone(), vec![], vec![("K=1".into(), "v".into())]),
        Err(McpConfigError::InvalidEnvName { .. })
    ));
    let error = new(
        "srv",
        abs.clone(),
        vec![],
        vec![("K\0".into(), MCP_SECRET.into())],
    )
    .unwrap_err();
    assert!(!error.to_string().contains(MCP_SECRET));

    // A server assembled through the public fields is re-validated before any spawn.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![McpStdioServer {
        name: "srv".into(),
        command: "relative/studio-mcp".into(),
        args: vec![],
        env: vec![("K".into(), MCP_SECRET.into())],
    }];
    let manager = ProcessTreeManager::new();
    let failure = AcpDriver::start(cfg, &manager).err().unwrap();
    assert_eq!(failure.kind, FailureKind::SpawnFailed);
    assert!(!format!("{failure:?}").contains(MCP_SECRET));
    assert_eq!(manager.active_count(), 0);
}
