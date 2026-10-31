//! The broker: capability security, expiry and revocation, queue, cancellation and connection limits.

use super::*;

#[test]
fn wrong_secret_and_unknown_capability_are_indistinguishable() {
    let h = harness();
    let cap = h.grant(&task());
    let zeros = "0".repeat(64);

    let mut wrong_secret = Wire::connect(&cap.socket);
    let first = wrong_secret.hello(&cap.capability, &zeros);
    assert!(
        wrong_secret.recv().is_none(),
        "connection closes after a failed hello"
    );

    let mut unknown = Wire::connect(&cap.socket);
    let second = unknown.hello("deadbeefdeadbeefdeadbeefdeadbeef", &cap.secret);
    assert!(unknown.recv().is_none());

    let mut garbled = Wire::connect(&cap.socket);
    garbled.send_raw("not json at all\n");
    let third = garbled.recv().unwrap();

    let mut short_secret = Wire::connect(&cap.socket);
    let fourth = short_secret.hello(&cap.capability, "abcd");

    for reply in [&first, &second, &third, &fourth] {
        assert_eq!(error_code(reply), "unauthorized");
        assert!(!reply.to_string().contains(&cap.secret));
    }
    assert_eq!(first, second, "no oracle for the capability id");
    assert_eq!(h.broker.stats().rejected_auth, 4);
    assert_eq!(h.backend.call_count(), 0);

    let mut good = cap.wire();
    assert_eq!(
        good.call(1, "timeline", json!({}))["result"]["result"]["tool"],
        "timeline"
    );
}

#[test]
fn capability_socket_and_runtime_dir_are_private() {
    let h = harness();
    let task = task();
    let grant = h
        .broker
        .grant(draft(&task), Duration::from_secs(60))
        .unwrap();
    #[cfg(unix)]
    {
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&h.runtime), 0o700);
        assert_eq!(mode(h.broker.socket_path()), 0o600);
        assert_eq!(mode(&grant.capability_file), 0o600);
    }
    // The DACLs themselves are asserted by the transport's unit tests.
    #[cfg(windows)]
    {
        let endpoint = h.broker.socket_path().to_str().unwrap();
        assert!(
            endpoint.starts_with(r"\\.\pipe\fframes-studio-"),
            "{endpoint}"
        );
        transport::open_private_file(&grant.capability_file).unwrap();
        assert!(!h.runtime.join("broker.sock").exists());
    }

    let cap = Cap::read(&grant.capability_file);
    let file_name = grant.capability_file.file_name().unwrap().to_str().unwrap();
    assert_eq!(file_name, format!("cap-{}.json", cap.capability));
    assert!(cap.socket.is_absolute());
    assert_eq!(cap.socket, h.broker.socket_path());
    assert_eq!(cap.secret.len(), 64);
    assert!(cap.secret.bytes().all(|b| b.is_ascii_hexdigit()));
    let value: Value = serde_json::from_slice(&fs::read(&cap.file).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value.as_object().unwrap().len(), 5);
    // `grant` lets the agent use every tool; `grant_tools` narrows the list.
    assert_eq!(
        value["tools"].as_array().unwrap().len(),
        studio_engine::agent_tools::TOOL_NAMES.len()
    );

    let other = h
        .broker
        .grant(draft(&task), Duration::from_secs(60))
        .unwrap();
    let other = Cap::read(&other.capability_file);
    assert_ne!(other.secret, cap.secret);
    assert_ne!(other.capability, cap.capability);

    for text in [
        format!("{grant:?}"),
        format!("{:?}", h.broker),
        format!("{:?}", h.broker.stats()),
    ] {
        assert!(
            !text.contains(&cap.secret),
            "secret leaked into Debug output"
        );
        assert!(
            !text.contains(&other.secret),
            "secret leaked into Debug output"
        );
    }
    assert_eq!(h.broker.stats().grants, 2);
}

#[test]
fn insecure_runtime_directories_are_refused() {
    let backend = FakeBackend::new();
    let liveness = Arc::new(FakeLiveness::default());
    let dir = short_tempdir();

    let created = dir.path().join("a").join("b");
    let broker = start_in(&created, &backend, &liveness).unwrap();
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&created).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(broker);

    #[cfg(windows)]
    {
        // Inherited from the temp directory: administrators and SYSTEM may open it.
        let shared = dir.path().join("shared");
        fs::create_dir(&shared).unwrap();
        let error = start_in(&shared, &backend, &liveness).unwrap_err();
        assert!(
            matches!(error, BrokerError::InsecureRuntimeDir { .. }),
            "{error}"
        );
        let junction = dir.path().join("junction");
        let made = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&created)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let error = start_in(&junction, &backend, &liveness).unwrap_err();
        assert!(
            matches!(error, BrokerError::InsecureRuntimeDir { .. }),
            "{error}"
        );
    }

    #[cfg(unix)]
    for (name, mode) in [("group", 0o750), ("world", 0o705), ("groupwrite", 0o770)] {
        let path = dir.path().join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let error = start_in(&path, &backend, &liveness).unwrap_err();
        assert!(
            matches!(error, BrokerError::InsecureRuntimeDir { .. }),
            "{name}: {error}"
        );
        assert!(
            !path.join("broker.sock").exists(),
            "{name}: nothing may be bound"
        );
    }

    #[cfg(unix)]
    {
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        let link = dir.path().join("link");
        symlink(&real, &link).unwrap();
        let error = start_in(&link, &backend, &liveness).unwrap_err();
        assert!(
            matches!(error, BrokerError::InsecureRuntimeDir { .. }),
            "{error}"
        );
    }

    let file = dir.path().join("file");
    fs::write(&file, b"x").unwrap();
    let error = start_in(&file, &backend, &liveness).unwrap_err();
    assert!(
        matches!(error, BrokerError::InsecureRuntimeDir { .. }),
        "{error}"
    );

    // Only unix socket addresses are length-limited.
    #[cfg(unix)]
    {
        let long = dir.path().join("x".repeat(120));
        let error = start_in(&long, &backend, &liveness).unwrap_err();
        assert!(
            matches!(error, BrokerError::SocketPathTooLong(_)),
            "{error}"
        );
    }
}

#[test]
fn a_second_broker_in_the_same_runtime_dir_is_refused_and_stale_files_are_cleared() {
    let h = harness();
    let error = start_in(&h.runtime, &h.backend, &h.liveness).unwrap_err();
    assert!(matches!(error, BrokerError::AlreadyRunning(_)), "{error}");
    // The refused start left the running broker untouched.
    let cap = h.grant(&task());
    assert_eq!(
        cap.wire().call(1, "timeline", json!({}))["result"]["revision"]["id"],
        REV_A
    );

    // A crashed broker leaves a socket file and capability files behind.
    let dir = short_tempdir();
    let runtime = dir.path().join("rt");
    let first = start_in(&runtime, &h.backend, &h.liveness).unwrap();
    let stale_cap = first
        .grant(draft(&task()), Duration::from_secs(60))
        .unwrap();
    std::mem::forget(first);
    assert!(stale_cap.capability_file.exists());
    // `forget` leaked a live broker (threads keep serving); a fresh start must refuse.
    let error = start_in(&runtime, &h.backend, &h.liveness).unwrap_err();
    assert!(matches!(error, BrokerError::AlreadyRunning(_)), "{error}");
}

#[test]
fn expiry_refuses_new_and_established_connections() {
    let h = harness();
    let task = task();
    let grant = h
        .broker
        .grant(draft(&task), Duration::from_millis(1500))
        .unwrap();
    let cap = Cap::read(&grant.capability_file);
    let mut established = cap.wire();
    assert!(
        established
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    thread::sleep(Duration::from_millis(1700));
    assert!(grant.expires_at < std::time::SystemTime::now());

    let mut fresh = Wire::connect(&cap.socket);
    let reply = fresh.hello(&cap.capability, &cap.secret);
    assert_eq!(
        error_code(&reply),
        "expired",
        "expiry is only revealed after the secret verified"
    );
    assert!(fresh.recv().is_none());

    let reply = established.call(2, "timeline", json!({}));
    assert_eq!(error_code(&reply), "expired");
    assert_eq!(reply["id"], 2);
    assert!(
        established.recv().is_none(),
        "the connection closes after expiry"
    );
    assert_eq!(h.backend.call_count(), 1);

    let mut wrong = Wire::connect(&cap.socket);
    let reply = wrong.hello(&cap.capability, &"1".repeat(64));
    assert_eq!(
        error_code(&reply),
        "unauthorized",
        "an expired capability does not reveal itself to a wrong secret"
    );
}

#[test]
fn revoke_refuses_reconnect_and_the_next_call() {
    let h = harness();
    let (one, two) = (task(), task());
    let cap_one = h.grant(&one);
    let cap_two = h.grant(&two);
    let mut established = cap_one.wire();
    let mut survivor = cap_two.wire();
    assert!(
        established
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    h.broker.revoke(&one);
    assert!(!cap_one.file.exists(), "the capability file is deleted");
    assert_eq!(h.broker.stats().grants, 1);

    let reply = established.call(2, "timeline", json!({}));
    assert_eq!(error_code(&reply), "expired");
    assert!(established.recv().is_none());

    let mut reconnect = Wire::connect(&cap_one.socket);
    let reply = reconnect.hello(&cap_one.capability, &cap_one.secret);
    assert_ne!(
        reply,
        json!({"ok": true}),
        "a revoked capability never reconnects"
    );
    assert!(reply.get("error").is_some());
    assert!(matches!(
        BrokerClient::connect(&cap_one.file),
        Err(ClientError::CapabilityFile(_))
    ));

    assert!(
        survivor
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}

#[test]
fn dead_tasks_are_stale_for_new_and_established_connections() {
    let h = harness();
    let task = task();
    let cap = h.grant(&task);
    let mut established = cap.wire();
    assert!(
        established
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    h.liveness.kill(&task);
    let reply = established.call(2, "timeline", json!({}));
    assert_eq!(error_code(&reply), "stale_task");
    assert!(established.recv().is_none());

    let mut fresh = Wire::connect(&cap.socket);
    let reply = fresh.hello(&cap.capability, &cap.secret);
    assert_eq!(error_code(&reply), "stale_task");
    assert_eq!(h.backend.call_count(), 1);
}

#[test]
fn oversized_request_lines_close_the_connection_without_executing() {
    let h = harness();
    let cap = h.grant(&task());

    let mut wire = cap.wire();
    let padding = "x".repeat(MAX_REQUEST_BYTES + 10);
    wire.send_raw(&format!(
        "{{\"id\":1,\"method\":\"timeline\",\"params\":{{\"project\":\"{padding}\"}}}}\n"
    ));
    let reply = wire.recv().expect("an error line before the close");
    assert_eq!(error_code(&reply), "invalid_params");
    assert!(wire.recv().is_none());

    // No newline at all: the broker must not buffer without bound.
    let mut endless = cap.wire();
    let mut writer = endless.stream.try_clone().unwrap();
    let flood = thread::spawn(move || {
        let chunk = vec![b'y'; 16 * 1024];
        for _ in 0..64 {
            if writer.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    let reply = endless.recv().expect("an error line before the close");
    assert_eq!(error_code(&reply), "invalid_params");
    assert!(endless.recv().is_none());
    flood.join().unwrap();

    // A line exactly at the limit is still a request.
    let mut exact = cap.wire();
    let head = "{\"id\":1,\"method\":\"timeline\",\"params\":{\"project\":\"";
    let tail = "\"}}";
    let fill = "p".repeat(MAX_REQUEST_BYTES - head.len() - tail.len());
    exact.send_raw(&format!("{head}{fill}{tail}\n"));
    let reply = exact.recv().unwrap();
    assert_eq!(
        error_code(&reply),
        "cross_project",
        "parsed and dispatched, not size-rejected"
    );
    assert_eq!(h.backend.call_count(), 0);
    assert_eq!(
        h.broker.stats().executed,
        1,
        "the at-limit call reached the dispatcher only"
    );
}

#[test]
fn queue_holds_sixteen_behind_two_workers_and_the_nineteenth_is_busy() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |_, request, cancelled| {
            gate.pass(cancelled);
            Ok(plain_reply(request))
        });
    }
    let cap = h.grant(&task());
    assert_eq!((MAX_QUEUED_CALLS, TOOL_WORKERS), (16, 2));

    let mut workers: Vec<Wire> = (0..2).map(|_| cap.wire()).collect();
    for (index, wire) in workers.iter_mut().enumerate() {
        wire.request(100 + index as u64, "timeline", json!({}));
    }
    gate.wait_entered(2);

    // Two pipelining connections queue 8 calls each: 16 waiting calls.
    let mut queued: Vec<Wire> = (0..2).map(|_| cap.wire()).collect();
    for wire in &mut queued {
        let batch: String = (1..=8)
            .map(|id| {
                format!(
                    "{}\n",
                    json!({"id": id, "method": "build_status", "params": {}})
                )
            })
            .collect();
        wire.send_raw(&batch);
    }
    wait_until("16 queued calls", || {
        h.broker.stats().queued_high_water == 16
    });

    let mut overflow = cap.wire();
    let started = Instant::now();
    let reply = overflow.call(7, "timeline", json!({}));
    assert_eq!(error_code(&reply), "busy");
    assert_eq!(reply["id"], 7);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "busy is immediate, not queued"
    );
    assert_eq!(
        h.backend.call_count(),
        2,
        "the busy call was not executed; the queue has not started"
    );

    gate.open();
    for (index, wire) in workers.iter_mut().enumerate() {
        let reply = wire.recv().unwrap();
        assert_eq!(reply["id"], 100 + index as u64);
        assert!(reply.get("result").is_some());
    }
    for wire in &mut queued {
        for id in 1..=8u64 {
            let reply = wire.recv().unwrap();
            assert_eq!(
                reply["id"], id,
                "one connection is answered strictly in order"
            );
            assert_eq!(reply["result"]["result"]["tool"], "build_status");
        }
    }
    // The overflow connection is still usable once the queue drained.
    assert!(
        overflow
            .call(8, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    let stats = h.broker.stats();
    assert_eq!(h.backend.max_running.load(Ordering::SeqCst), TOOL_WORKERS);
    assert_eq!(stats.queued_high_water, MAX_QUEUED_CALLS);
    assert_eq!(stats.rejected_busy, 1);
    assert_eq!(stats.executed, 19);
}

#[test]
fn requests_on_one_connection_run_in_order() {
    let h = harness();
    let order = Arc::new(Mutex::new(Vec::new()));
    {
        let order = order.clone();
        h.backend.set(move |_, request, _| {
            if let ToolCall::RenderFrame { frame, .. } = request.call {
                // Earlier calls are slower: only ordering keeps the sequence.
                thread::sleep(Duration::from_millis(60 - 10 * frame as u64));
                order.lock().push(frame);
            }
            Ok(plain_reply(request))
        });
    }
    let mut wire = h.grant(&task()).wire();
    let batch: String = (0..5u64)
        .map(|frame| {
            format!(
                "{}\n",
                json!({"id": frame + 1, "method": "render_frame", "params": {"frame": frame}})
            )
        })
        .collect();
    wire.send_raw(&batch);
    for id in 1..=5u64 {
        assert_eq!(wire.recv().unwrap()["id"], id);
    }
    assert_eq!(*order.lock(), vec![0, 1, 2, 3, 4]);
}

#[test]
fn a_dropped_connection_cancels_its_executing_call_and_queued_calls() {
    let h = harness();
    let gate = Gate::new();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    {
        let (gate, saw_cancel) = (gate.clone(), saw_cancel.clone());
        h.backend.set(move |_, _, cancelled| {
            if gate.pass(cancelled) {
                return Err(ToolError::new(ToolErrorCode::Internal, "gate opened"));
            }
            saw_cancel.store(true, Ordering::SeqCst);
            Err(ToolError::new(ToolErrorCode::Cancelled, "cancelled"))
        });
    }
    let cap = h.grant(&task());
    let mut wire = cap.wire();
    wire.send_raw(&format!(
        "{}\n{}\n",
        json!({"id": 1, "method": "timeline", "params": {}}),
        json!({"id": 2, "method": "build_status", "params": {}})
    ));
    gate.wait_entered(1);
    drop(wire);
    wait_until("cancellation of the executing call", || {
        saw_cancel.load(Ordering::SeqCst)
    });
    wait_until("connection teardown", || {
        h.broker.stats().live_connections == 0
    });
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        h.backend.call_count(),
        1,
        "the call queued behind it never ran"
    );
    assert_eq!(h.backend.running.load(Ordering::SeqCst), 0);

    // The workers are free again.
    h.backend.set(|_, request, _| Ok(plain_reply(request)));
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}

#[test]
fn revoke_cancels_an_executing_call() {
    let h = harness();
    let gate = Gate::new();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    {
        let (gate, saw_cancel) = (gate.clone(), saw_cancel.clone());
        h.backend.set(move |_, _, cancelled| {
            if !gate.pass(cancelled) {
                saw_cancel.store(true, Ordering::SeqCst);
            }
            Err(ToolError::new(ToolErrorCode::Cancelled, "cancelled"))
        });
    }
    let task = task();
    let mut wire = h.grant(&task).wire();
    wire.request(1, "timeline", json!({}));
    gate.wait_entered(1);
    h.broker.revoke(&task);
    wait_until("cancellation by revoke", || {
        saw_cancel.load(Ordering::SeqCst)
    });
    let reply = wire.recv().unwrap();
    assert_eq!(reply["id"], 1);
    assert!(reply.get("error").is_some());
}

#[test]
fn shutdown_cancels_joins_and_removes_socket_and_capabilities() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |_, _, cancelled| {
            gate.pass(cancelled);
            Err(ToolError::new(ToolErrorCode::Cancelled, "shutdown"))
        });
    }
    let task = task();
    let cap = h.grant(&task);
    let second = h.grant(&task);
    let mut wire = cap.wire();
    wire.request(1, "timeline", json!({}));
    gate.wait_entered(1);
    assert!(h.broker.stats().live_threads >= TOOL_WORKERS + 2);
    let socket = h.broker.socket_path().to_path_buf();
    assert!(endpoint_exists(&socket));

    h.broker.shutdown();
    assert!(!endpoint_exists(&socket), "the socket file is removed");
    assert!(
        !cap.file.exists() && !second.file.exists(),
        "capability files are removed"
    );
    let stats = h.broker.stats();
    assert_eq!(stats.live_threads, 0, "every broker thread was joined");
    assert_eq!(stats.live_connections, 0);
    assert_eq!(stats.grants, 0);
    assert_eq!(
        h.backend.running.load(Ordering::SeqCst),
        0,
        "the in-flight call was cancelled"
    );
    assert!(wire.recv().is_none(), "established connections are closed");
    assert!(raw_connect(&socket).is_err());
    assert!(matches!(
        h.broker.grant(draft(&task), Duration::from_secs(1)),
        Err(BrokerError::ShutDown)
    ));
    h.broker.shutdown();
    assert!(
        h.runtime.exists(),
        "the runtime directory itself is the app's"
    );
}

#[test]
fn dropping_the_broker_shuts_it_down() {
    let dir = short_tempdir();
    let runtime = dir.path().join("rt");
    let backend = FakeBackend::new();
    let liveness = Arc::new(FakeLiveness::default());
    let broker = start_in(&runtime, &backend, &liveness).unwrap();
    let grant = broker
        .grant(draft(&task()), Duration::from_secs(60))
        .unwrap();
    let socket = broker.socket_path().to_path_buf();
    let _held = Cap::read(&grant.capability_file).wire();
    drop(broker);
    assert!(!endpoint_exists(&socket));
    assert!(!grant.capability_file.exists());
    assert_eq!(
        fs::read_dir(&runtime).unwrap().count(),
        0,
        "nothing is left behind"
    );
    // The same runtime directory can host a new broker straight away.
    let again = start_in(&runtime, &backend, &liveness).unwrap();
    assert!(endpoint_exists(again.socket_path()));
}

#[test]
fn a_panicking_backend_is_an_internal_error_and_the_broker_keeps_serving() {
    let h = harness();
    h.backend.set(|binding, request, cancelled| {
        if let ToolCall::RenderFrame { frame: 13, .. } = request.call {
            panic!("backend exploded");
        }
        standard(binding, request, cancelled)
    });
    let cap = h.grant(&task());
    let mut wire = cap.wire();
    for id in 1..=(TOOL_WORKERS as u64 + 2) {
        let reply = wire.call(id, "render_frame", json!({"frame": 13}));
        assert_eq!(error_code(&reply), "internal");
        assert_eq!(reply["id"], id);
    }
    assert!(wire.call(10, "timeline", json!({})).get("result").is_some());
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
    assert_eq!(h.backend.running.load(Ordering::SeqCst), 0);
    assert!(
        h.broker.stats().live_threads > TOOL_WORKERS,
        "workers and acceptor survived"
    );
}

#[test]
fn unknown_methods_and_bad_params_are_answered_without_closing() {
    let h = harness();
    let mut wire = h.grant(&task()).wire();
    assert_eq!(
        error_code(&wire.call(1, "shell", json!({}))),
        "method_not_found"
    );
    assert_eq!(
        error_code(&wire.call(2, "render_frame", json!({"frame": "x"}))),
        "invalid_params"
    );
    wire.send_raw("[1,2]\n");
    assert_eq!(error_code(&wire.recv().unwrap()), "invalid_params");
    wire.send_raw(
        "{\"id\":9,\"method\":\"render_frame\",\"params\":{\"frame\":1,\"scale\":NaN}}\n",
    );
    assert_eq!(error_code(&wire.recv().unwrap()), "invalid_params");
    wire.send_raw("{\"method\":\"timeline\"}\n");
    assert_eq!(error_code(&wire.recv().unwrap()), "invalid_params");
    assert!(wire.call(3, "timeline", json!({})).get("result").is_some());
    assert_eq!(h.backend.call_count(), 1);
}

#[test]
fn hello_must_arrive_within_five_seconds() {
    let h = harness();
    let cap = h.grant(&task());
    let mut silent = Wire::connect(&cap.socket);
    let started = Instant::now();
    assert!(
        silent.recv().is_none(),
        "the broker closes a connection that never says hello"
    );
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(4500),
        "closed too early: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(9),
        "closed too late: {waited:?}"
    );
    assert_eq!(h.broker.stats().rejected_auth, 1);
}

#[test]
fn at_most_eight_connections_are_served() {
    let h = harness();
    let cap = h.grant(&task());
    let mut wires: Vec<Wire> = (0..8).map(|_| cap.wire()).collect();
    wait_until("8 live connections", || {
        h.broker.stats().live_connections == 8
    });

    let mut ninth = Wire::connect(&cap.socket);
    let reply = ninth.recv().expect("a refusal line");
    assert_eq!(error_code(&reply), "busy");
    assert!(ninth.recv().is_none());

    drop(wires.pop());
    wait_until("a free connection slot", || {
        h.broker.stats().live_connections == 7
    });
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}

/// A client that the pipe's DACL lets in (its restricting SIDs include the user) but whose
/// token is restricted is refused by the broker's own check of the client token.
#[cfg(windows)]
#[test]
fn a_client_under_a_restricted_token_is_refused_by_the_token_check() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{
            CreateRestrictedToken, CreateWellKnownSid, GetTokenInformation, SECURITY_MAX_SID_SIZE,
            SID_AND_ATTRIBUTES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_QUERY,
            TOKEN_USER, TokenGroups, TokenUser, WinRestrictedCodeSid,
        },
        System::SystemServices::{SE_GROUP_ENABLED, SE_GROUP_INTEGRITY},
        System::Threading::{
            CREATE_NO_WINDOW, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess,
            OpenProcessToken, PROCESS_INFORMATION, STARTUPINFOW, WaitForSingleObject,
        },
    };

    let h = harness();
    let cap = h.grant(&task());
    // SAFETY: plain Win32 calls with valid out-parameters; every handle is closed below.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        assert_ne!(
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
                &mut token,
            ),
            0
        );
        let mut user = vec![0u64; 64];
        let mut length = 0u32;
        assert_ne!(
            GetTokenInformation(
                token,
                TokenUser,
                user.as_mut_ptr().cast(),
                (user.len() * 8) as u32,
                &mut length,
            ),
            0
        );
        let user_sid = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        // Restricting SIDs: the user, every enabled group of this token and RESTRICTED.
        // The child can then open whatever this process can (its DLLs, the window
        // station, the pipe), whatever the runner's folder permissions, yet its token is
        // still restricted.
        let mut size = 0u32;
        GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut size);
        let mut groups = vec![0u64; size.div_ceil(8) as usize];
        assert_ne!(
            GetTokenInformation(
                token,
                TokenGroups,
                groups.as_mut_ptr().cast(),
                (groups.len() * 8) as u32,
                &mut size,
            ),
            0
        );
        let groups = &*groups.as_ptr().cast::<TOKEN_GROUPS>();
        let groups = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        let mut restricted_code = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut code_size = restricted_code.len() as u32;
        assert_ne!(
            CreateWellKnownSid(
                WinRestrictedCodeSid,
                std::ptr::null_mut(),
                restricted_code.as_mut_ptr().cast(),
                &mut code_size
            ),
            0
        );
        let restricting: Vec<SID_AND_ATTRIBUTES> = std::iter::once(user_sid)
            .chain(
                groups
                    .iter()
                    .filter(|group| {
                        group.Attributes & SE_GROUP_ENABLED as u32 != 0
                            && group.Attributes & SE_GROUP_INTEGRITY as u32 == 0
                    })
                    .map(|group| group.Sid),
            )
            .chain(Some(restricted_code.as_mut_ptr().cast()))
            .map(|sid| SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: 0,
            })
            .collect();
        let mut restricted: HANDLE = std::ptr::null_mut();
        assert_ne!(
            CreateRestrictedToken(
                token,
                0,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                restricting.len() as u32,
                restricting.as_ptr(),
                &mut restricted,
            ),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        let command = format!(
            "\"{}\" --capability \"{}\" project_context",
            env!("CARGO_BIN_EXE_studio-tools"),
            cap.file.display()
        );
        let mut command: Vec<u16> = std::ffi::OsStr::new(&command)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..std::mem::zeroed()
        };
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        assert_ne!(
            CreateProcessAsUserW(
                restricted,
                std::ptr::null(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW,
                std::ptr::null(),
                std::ptr::null(),
                &startup,
                &mut process,
            ),
            0,
            "{}",
            std::io::Error::last_os_error()
        );
        WaitForSingleObject(process.hProcess, 30_000);
        let mut code = 0u32;
        assert_ne!(GetExitCodeProcess(process.hProcess, &mut code), 0);
        for handle in [process.hProcess, process.hThread, restricted, token] {
            CloseHandle(handle);
        }
        assert_eq!(
            code, 3,
            "studio-tools must report the broker as unavailable"
        );
    }
    wait_until("the restricted client is refused", || {
        h.broker.stats().rejected_auth == 1
    });
    assert_eq!(h.backend.call_count(), 0);
    // The unrestricted user still gets through.
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}
