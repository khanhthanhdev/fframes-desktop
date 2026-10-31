//! The `studio-tools` and `studio-mcp` subprocesses: CLI/MCP parity, exit codes, flags and bounds.

use super::*;

struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cli(cap: Option<&Path>, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_studio-tools"));
    command.env_remove(CAPABILITY_ENV);
    if let Some(cap) = cap {
        command.arg("--capability").arg(cap);
    }
    command.args(args);
    command
}

struct Finished {
    code: i32,
    stdout: String,
    stderr: String,
}

fn finish(command: &mut Command) -> Finished {
    let output = command.output().unwrap();
    Finished {
        code: output.status.code().expect("exited normally"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

#[cfg(target_os = "linux")]
fn cmdline(child: &Child) -> String {
    let bytes = fs::read(format!("/proc/{}/cmdline", child.id())).expect("child is alive");
    String::from_utf8_lossy(&bytes).replace('\0', " ")
}

#[cfg(target_os = "macos")]
fn cmdline(child: &Child) -> String {
    let output = Command::new("ps")
        .args(["-p", &child.id().to_string(), "-o", "command="])
        .output()
        .expect("ps is available");
    assert!(output.status.success(), "ps could not inspect child argv");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(windows)]
fn cmdline(child: &Child) -> String {
    let query = format!(
        "(Get-CimInstance Win32_Process -Filter 'ProcessId={}').CommandLine",
        child.id()
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &query])
        .output()
        .expect("powershell is available");
    assert!(
        output.status.success(),
        "powershell could not inspect child argv"
    );
    let line = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    assert!(!line.is_empty(), "the child is alive");
    line
}

struct McpChild {
    child: Reaped,
    stdin: Option<std::process::ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Option<thread::JoinHandle<Vec<u8>>>,
}

impl McpChild {
    fn spawn(cap: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_studio-mcp"));
        command
            .env_remove(CAPABILITY_ENV)
            .arg("--capability")
            .arg(cap)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        Self {
            child: Reaped(child),
            stdin,
            lines,
            stderr: Some(stderr),
        }
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    }

    fn recv_line(&mut self) -> String {
        self.lines.recv_timeout(WAIT).expect("an MCP response line")
    }

    fn recv(&mut self) -> Value {
        let line = self.recv_line();
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("stdout carried a non-JSON line {line:?}: {error}"));
        assert_eq!(
            value["jsonrpc"], "2.0",
            "stdout carries JSON-RPC only: {line}"
        );
        value
    }

    /// Close stdin, expect a clean exit and return what is left on stdout/stderr.
    fn close(mut self) -> (Vec<String>, Vec<u8>) {
        drop(self.stdin.take());
        let status = self.child.0.wait().unwrap();
        assert_eq!(status.code(), Some(0), "stdin EOF is a clean exit");
        let rest: Vec<String> = self.lines.iter().collect();
        let stderr = self.stderr.take().unwrap().join().unwrap();
        (rest, stderr)
    }
}

#[test]
fn cli_and_mcp_return_byte_identical_replies_without_the_secret_on_argv() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |binding, request, cancelled| {
            if matches!(request.call, ToolCall::BuildStatus) {
                gate.pass(cancelled);
            }
            standard(binding, request, cancelled)
        });
    }
    let cap = h.grant(&task());

    // CLI: the artifact reply and the error case.
    let args = r#"{"frame":3,"scale":0.25}"#;
    let ok = finish(&mut cli(Some(&cap.file), &["render_frame", "--json", args]));
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert!(ok.stderr.is_empty());
    assert!(
        ok.stdout.ends_with('\n') && ok.stdout.matches('\n').count() == 1,
        "one compact line"
    );
    let cli_text = ok.stdout.trim_end_matches('\n').to_string();
    let cli_value: Value = serde_json::from_str(&cli_text).unwrap();
    assert_eq!(cli_value["artifacts"][0]["media_type"], "image/png");
    assert_eq!(
        cli_text,
        serde_json::to_string(&cli_value).unwrap(),
        "compact JSON"
    );

    let failed = finish(&mut cli(
        Some(&cap.file),
        &["render_frame", "--json", r#"{"frame":999}"#],
    ));
    assert_eq!(failed.code, 1);
    assert!(failed.stdout.is_empty());
    let cli_error: Value = serde_json::from_str(failed.stderr.trim_end()).unwrap();
    assert_eq!(cli_error["error"]["code"], "not_found");

    // The same calls through MCP.
    let mut mcp = McpChild::spawn(&cap.file);
    mcp.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}));
    let init = mcp.recv();
    assert_eq!(init["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    mcp.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    mcp.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = mcp.recv();
    assert_eq!(list["id"], 2, "the notification produced no output");
    let listed: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, TOOL_NAMES);

    let argv = cmdline(&mcp.child.0);
    assert!(
        !argv.contains(&cap.secret),
        "the secret must never be on argv"
    );
    assert!(
        argv.contains(cap.file.to_str().unwrap()),
        "only the capability file path is passed"
    );

    mcp.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"render_frame","arguments":serde_json::from_str::<Value>(args).unwrap()}}));
    let call = mcp.recv();
    assert_eq!(call["id"], 3);
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(call["result"]["structuredContent"], cli_value);
    assert_eq!(
        call["result"]["content"][0]["text"].as_str().unwrap(),
        cli_text,
        "byte-identical compact text"
    );
    assert_eq!(
        serde_json::to_string(&call["result"]["structuredContent"]).unwrap(),
        cli_text
    );

    mcp.send(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"render_frame","arguments":{"frame":999}}}));
    let error = mcp.recv();
    assert_eq!(error["result"]["isError"], true);
    let error_text: Value =
        serde_json::from_str(error["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        error_text, cli_error["error"],
        "CLI stderr and MCP isError carry the same error"
    );

    mcp.send(
        &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"launch_missiles"}}),
    );
    assert_eq!(mcp.recv()["error"]["code"], INVALID_PARAMS);
    mcp.send(&json!({"jsonrpc":"2.0","id":6,"method":"ping"}));
    assert_eq!(mcp.recv()["result"], json!({}));
    mcp.send(&json!({"jsonrpc":"2.0","id":7,"method":"bogus"}));
    assert_eq!(mcp.recv()["error"]["code"], METHOD_NOT_FOUND);
    mcp.stdin
        .as_mut()
        .unwrap()
        .write_all(b"this is not json\n")
        .unwrap();
    let parse = mcp.recv();
    assert_eq!(parse["error"]["code"], PARSE_ERROR);
    assert_eq!(parse["id"], Value::Null);
    let (rest, stderr) = mcp.close();
    assert!(
        rest.is_empty(),
        "stdout carried nothing beyond the responses: {rest:?}"
    );
    assert!(stderr.len() <= 16 * 1024, "stderr stays bounded");
    assert!(!String::from_utf8_lossy(&stderr).contains(&cap.secret));
    assert!(
        String::from_utf8_lossy(&stderr).contains("unqualified"),
        "the qualification is stated at startup"
    );

    // The CLI's argv while it runs (held inside the backend by the gate).
    let mut command = cli(Some(&cap.file), &["build_status"]);
    let args_for_secret_check: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(
        args_for_secret_check
            .iter()
            .all(|arg| !arg.contains(&cap.secret))
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = Reaped(command.spawn().unwrap());
    gate.wait_entered(1);
    let argv = cmdline(&child.0);
    assert!(
        !argv.contains(&cap.secret),
        "the secret must never be on argv"
    );
    assert!(argv.contains("build_status"));
    gate.open();
    let output = {
        let mut child = child;
        let status = child.0.wait().unwrap();
        let mut out = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        (status.code(), out)
    };
    assert_eq!(output.0, Some(0));
    let value: Value = serde_json::from_str(output.1.trim_end()).unwrap();
    assert_eq!(value["result"]["tool"], "build_status");
}

#[test]
fn cli_exit_codes_and_list() {
    let h = harness();
    let task = task();
    let cap = h.grant(&task);

    let ok = finish(&mut cli(Some(&cap.file), &["timeline"]));
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&ok.stdout).unwrap()["result"]["tool"],
        "timeline"
    );

    // The capability path may come from the environment.
    let via_env = finish(cli(None, &["project_context"]).env(CAPABILITY_ENV, &cap.file));
    assert_eq!(via_env.code, 0, "{}", via_env.stderr);

    let listing = finish(&mut cli(None, &["--list"]));
    assert_eq!(listing.code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&listing.stdout).unwrap(),
        Value::Array(tool_descriptions())
    );
    assert_eq!(finish(&mut cli(None, &["--help"])).code, 0);

    // 1: a tool error.
    for params in [r#"{"frame":999}"#, r#"{"frame":1,"scale":9}"#] {
        let failed = finish(&mut cli(
            Some(&cap.file),
            &["render_frame", "--json", params],
        ));
        assert_eq!(failed.code, 1, "{params}");
        assert!(failed.stdout.is_empty());
        let error: Value = serde_json::from_str(failed.stderr.trim_end()).unwrap();
        assert!(error["error"]["code"].is_string() && error["error"]["message"].is_string());
    }

    // 2: usage errors.
    for args in [
        vec![],
        vec!["launch_missiles"],
        vec!["timeline", "extra"],
        vec!["render_frame", "--json", "{not json"],
        vec!["render_frame", "--json", "[1]"],
        vec!["render_frame", "--json"],
        vec!["--bogus"],
        vec!["--list", "timeline"],
    ] {
        let usage = finish(&mut cli(Some(&cap.file), &args));
        assert_eq!(usage.code, 2, "{args:?}: {}", usage.stderr);
        assert!(usage.stdout.is_empty());
    }
    let no_capability = finish(&mut cli(None, &["timeline"]));
    assert_eq!(no_capability.code, 2);
    assert!(no_capability.stderr.contains(CAPABILITY_ENV));

    // 3: broker unreachable / capability unusable.
    let dir = short_tempdir();
    let missing = dir.path().join("missing.json");
    let unusable = finish(&mut cli(Some(&missing), &["timeline"]));
    assert_eq!(unusable.code, 3);
    let error: Value = serde_json::from_str(unusable.stderr.trim_end()).unwrap();
    assert_eq!(error["error"]["code"], "unavailable");
    let readable = private_copy(
        dir.path(),
        "readable.json",
        &fs::read(&cap.file).unwrap(),
        0o644,
    );
    assert_eq!(finish(&mut cli(Some(&readable), &["timeline"])).code, 3);
    let copy = private_copy(
        dir.path(),
        "copy.json",
        &fs::read(&cap.file).unwrap(),
        0o600,
    );

    h.liveness.kill(&task);
    let stale = finish(&mut cli(Some(&cap.file), &["timeline"]));
    assert_eq!(stale.code, 3, "a rejected capability is unusable");
    assert!(stale.stderr.contains("stale_task"));

    h.broker.shutdown();
    let down = finish(&mut cli(Some(&copy), &["timeline"]));
    assert_eq!(down.code, 3, "{}", down.stderr);
    assert!(!down.stderr.contains(&cap.secret));
}

#[test]
fn mcp_flags_and_a_missing_broker() {
    let protocol = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .arg("--protocol")
        .output()
        .unwrap();
    assert_eq!(protocol.status.code(), Some(0));
    let text = String::from_utf8(protocol.stdout).unwrap();
    assert!(text.contains(MCP_PROTOCOL_VERSION));
    assert!(text.contains(MCP_QUALIFICATION));
    assert!(MCP_QUALIFICATION.starts_with("unqualified"));

    let usage = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .env_remove(CAPABILITY_ENV)
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    let usage = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .arg("--bogus")
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    // stdout carries JSON-RPC only: a usage error is a bounded stderr message.
    assert!(usage.stdout.is_empty());
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage: studio-mcp"));

    // initialize/tools/list work with no broker; a tool call reports it as a tool error.
    let dir = short_tempdir();
    let mut mcp = McpChild::spawn(&dir.path().join("no-such-capability.json"));
    mcp.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}));
    assert!(mcp.recv().get("result").is_some());
    mcp.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert_eq!(mcp.recv()["result"]["tools"].as_array().unwrap().len(), 9);
    mcp.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"timeline"}}));
    let call = mcp.recv();
    assert_eq!(call["result"]["isError"], true);
    let error: Value =
        serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["code"], "unavailable");
    // A line over the limit is rejected and the stream resynchronizes at the newline.
    let mut oversized = vec![b'z'; MAX_MCP_LINE_BYTES + 100];
    oversized.push(b'\n');
    mcp.stdin.as_mut().unwrap().write_all(&oversized).unwrap();
    assert_eq!(mcp.recv()["error"]["code"], INVALID_REQUEST);
    mcp.send(&json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
    assert_eq!(mcp.recv()["id"], 4);
    let (rest, _) = mcp.close();
    assert!(rest.is_empty());
}

#[test]
fn mcp_stderr_is_bounded_to_sixteen_kib() {
    let dir = short_tempdir();
    let mut mcp = McpChild::spawn(&dir.path().join("no-such-capability.json"));
    let mut stdin = mcp.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        let mut requests = vec![
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}),
        ];
        requests.extend((1..=700).map(|id| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"timeline"}})));
        for request in requests {
            if writeln!(stdin, "{request}").is_err() {
                return;
            }
        }
    });
    for id in 0..=700u64 {
        assert_eq!(mcp.recv()["id"], id);
    }
    writer.join().unwrap();
    let (rest, stderr) = mcp.close();
    assert!(rest.is_empty());
    assert!(
        stderr.len() <= 16 * 1024,
        "stderr was {} bytes",
        stderr.len()
    );
    assert!(String::from_utf8_lossy(&stderr).contains("log limit reached"));
}

#[test]
fn grants_are_short_lived_and_expired_ones_do_not_linger() {
    use studio_engine::agent_tools::broker::MAX_GRANT_TTL;
    assert!(MAX_GRANT_TTL <= Duration::from_secs(60 * 60));
    let h = harness();
    // A request for ten days is clamped to the maximum lifetime.
    let long = h
        .broker
        .grant(draft(&task()), Duration::from_secs(10 * 24 * 60 * 60))
        .unwrap();
    let latest = std::time::SystemTime::now() + MAX_GRANT_TTL + Duration::from_secs(5);
    assert!(long.expires_at <= latest, "ttl must be clamped");

    let brief = h
        .broker
        .grant(draft(&task()), Duration::from_millis(300))
        .unwrap();
    assert!(brief.capability_file.is_file());
    thread::sleep(Duration::from_millis(450));
    // The next grant sweeps the expired capability and its secret-bearing file.
    let fresh = h.grant(&task());
    assert!(!brief.capability_file.exists(), "expired file was swept");
    assert!(long.capability_file.is_file());
    assert!(fresh.file.is_file());
    assert_eq!(h.broker.stats().grants, 2);
}

#[test]
fn initialize_answers_the_one_revision_this_server_implements_whatever_the_client_asks() {
    assert_eq!(MCP_PROTOCOL_VERSION, "2025-06-18");
    for asked in ["2025-11-25", "2025-06-18", "2024-11-05", "nonsense"] {
        let mut server = McpServer::new(|| Err(ToolError::new(ToolErrorCode::Unavailable, "n/a")));
        let reply = rpc(
            &mut server,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion": asked, "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"}}}),
        );
        assert_eq!(
            reply["result"]["protocolVersion"], MCP_PROTOCOL_VERSION,
            "client asked {asked}"
        );
        assert_eq!(
            reply["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
    }
}

#[test]
fn turned_off_tools_are_left_out_of_the_list_and_refused_on_every_route() {
    let h = harness();
    let task = task();
    let file = h
        .broker
        .grant_tools(
            draft(&task),
            Duration::from_secs(600),
            &["timeline", "build_status"],
        )
        .unwrap()
        .capability_file;

    // The command line reaches the broker, which refuses a tool the user turned off.
    let refused = finish(&mut cli(
        Some(&file),
        &["render_frame", "--json", r#"{"frame":3}"#],
    ));
    assert_eq!(refused.code, 1, "{}", refused.stderr);
    let error: Value = serde_json::from_str(refused.stderr.trim_end()).unwrap();
    assert_eq!(error["error"]["code"], "method_not_found");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("turned off")
    );
    assert_eq!(finish(&mut cli(Some(&file), &["timeline"])).code, 0);

    // MCP lists only the allowed tools and refuses the others itself.
    let mut mcp = McpChild::spawn(&file);
    mcp.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}));
    mcp.recv();
    mcp.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = mcp.recv();
    let listed: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, ["timeline", "build_status"]);
    mcp.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"render_frame","arguments":{"frame":3}}}));
    let call = mcp.recv();
    assert_eq!(call["error"]["code"], -32602, "{call}");
    assert!(
        call["error"]["message"]
            .as_str()
            .unwrap()
            .contains("turned off")
    );
}
