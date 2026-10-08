//! The production GPUI shell as a real process on a private X server.
//!
//! GPUI's headless test support (`TestAppContext`) needs the `test-support` feature, which
//! pulls git-only dependencies that are not in this workspace's lockfile, so the shell's
//! hand-off wiring is exercised the way the M0/M2 native qualifications were: the real
//! `fframes-studio qualify-m3` binary on an owned Xvfb, driven with xdotool, observed
//! through its redacted telemetry file. It covers what pure tests cannot: the prompt's
//! native input path, `StudioShell::adopt_promotion` adopting the staged preview on the UI
//! thread, the fresh audio epoch and the matching video/audio install, the accepted-awaiting
//! label clearing exactly when the matching preview shows, and Undo's hand-off back.
//!
//! The agent is the scripted ACP peer: this proves the shell and the workflow, never a
//! provider. Run explicitly:
//!
//! ```text
//! SDK_ACTIVE=<installed SDK dir ending in .fframes/sdk/active> \
//!     cargo test --locked -p fframes-studio --test x11_shell -- --ignored --nocapture
//! ```
//!
//! Requires `Xvfb` and `xdotool`. The window is 1280x800 at the origin, and the click
//! positions below are that layout's controls; a layout change must update them (the test
//! then fails loudly, it never passes by accident).
#![cfg(unix)]

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use studio_engine::build_materialization::sdk_pin;
use studio_sdk::{CompatibilityManifest, SdkInstaller};

#[path = "support/preview_fixture.rs"]
#[allow(dead_code)]
mod preview_fixture;

const WAIT: Duration = Duration::from_secs(240);

/// Both native tests build with the same SDK and cache directories: one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

const CHAT_TAB: (u32, u32) = (906, 162);
const PROJECT_TAB: (u32, u32) = (969, 162);
const UNDO_BUTTON: (u32, u32) = (1000, 470);

struct Owned(Child);

impl Drop for Owned {
    fn drop(&mut self) {
        // Only the processes this test spawned: SIGTERM its process group, then reap.
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGTERM) };
        for _ in 0..50 {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
        let _ = self.0.wait();
    }
}

fn spawn(command: &mut Command) -> Owned {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    Owned(command.spawn().expect("spawn"))
}

/// The description names the interpreter by absolute path (the global PATH is not consulted).
fn python3() -> String {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("python3"))
        .find(|candidate| candidate.is_file())
        .expect("python3 on PATH")
        .to_string_lossy()
        .into_owned()
}

fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("-help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn free_display() -> u32 {
    (130..200)
        .find(|n| {
            !Path::new(&format!("/tmp/.X{n}-lock")).exists()
                && !Path::new(&format!("/tmp/.X11-unix/X{n}")).exists()
        })
        .expect("a free X display number")
}

struct Screen {
    display: String,
    _server: Owned,
}

impl Screen {
    fn start() -> Self {
        let number = free_display();
        let display = format!(":{number}");
        let server = spawn(
            Command::new("Xvfb")
                .args([
                    display.as_str(),
                    "-screen",
                    "0",
                    "1280x800x24",
                    "-nolisten",
                    "tcp",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        );
        let socket = format!("/tmp/.X11-unix/X{number}");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !Path::new(&socket).exists() {
            assert!(Instant::now() < deadline, "Xvfb did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Self {
            display,
            _server: server,
        }
    }

    fn xdotool(&self, args: &[&str]) {
        let status = Command::new("xdotool")
            .env("DISPLAY", &self.display)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("xdotool");
        assert!(status.success(), "xdotool {args:?}");
    }

    fn window(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let output = Command::new("xdotool")
                .env("DISPLAY", &self.display)
                .args(["search", "--name", "fframes Studio"])
                .stderr(Stdio::null())
                .output()
                .expect("xdotool");
            if let Some(id) = String::from_utf8_lossy(&output.stdout).lines().next() {
                return id.to_owned();
            }
            assert!(Instant::now() < deadline, "no Studio window");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn click(&self, window: &str, at: (u32, u32)) {
        self.xdotool(&[
            "mousemove",
            "--window",
            window,
            &at.0.to_string(),
            &at.1.to_string(),
            "click",
            "1",
        ]);
    }
}

struct Telemetry(PathBuf);

impl Telemetry {
    fn read(&self) -> Option<Value> {
        serde_json::from_slice(&fs::read(&self.0).ok()?).ok()
    }

    /// Waits until `key(telemetry)` is present and unchanged for 1.5 s. Measured bounds
    /// move while the layout settles (status lines appear above the controls); clicking a
    /// bound read once, mid-layout, lands on whatever is there by then.
    fn settled(&self, what: &str, key: impl Fn(&Value) -> Value) -> Value {
        let deadline = Instant::now() + WAIT;
        let mut stable: Option<(Value, Instant)> = None;
        loop {
            if let Some(value) = self.read() {
                let now = key(&value);
                if !now.is_null() {
                    match &stable {
                        Some((seen, since)) if *seen == now => {
                            if since.elapsed() >= Duration::from_millis(1500) {
                                return value;
                            }
                        }
                        _ => stable = Some((now, Instant::now())),
                    }
                } else {
                    stable = None;
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what} to settle; last telemetry: {:?}",
                self.read()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait(&self, what: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(value) = self.read()
                && predicate(&value)
            {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; last telemetry: {:?}",
                self.read()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The shell button's centre from the telemetry's measured bounds; a missing button fails
/// loudly instead of clicking a guessed position.
fn button_centre(value: &Value, name: &str) -> (u32, u32) {
    let bounds = value["buttons"][name]
        .as_array()
        .unwrap_or_else(|| panic!("no measured bounds for the {name} button: {value}"));
    let at = |i: usize| bounds[i].as_f64().expect("finite bounds");
    (
        (at(0) + at(2) / 2.).round() as u32,
        (at(1) + at(3) / 2.).round() as u32,
    )
}

fn click_prompt(screen: &Screen, window: &str, telemetry: &Telemetry) {
    let measured = telemetry.settled("the prompt input bounds", |v| {
        v["panel"]["prompt_bounds"].clone()
    });
    let bounds = measured["panel"]["prompt_bounds"]
        .as_array()
        .expect("measured prompt bounds");
    let at = |i: usize| bounds[i].as_f64().expect("finite prompt bounds");
    screen.click(
        window,
        (
            (at(0) + at(2) / 2.).round() as u32,
            (at(1) + at(3) / 2.).round() as u32,
        ),
    );
}

fn displayed(value: &Value) -> &Value {
    &value["preview"]["displayed"]
}

fn phase(value: &Value) -> &str {
    value["panel"]["phase"].as_str().unwrap_or("")
}

#[test]
#[ignore = "requires SDK_ACTIVE, Xvfb and xdotool; real Cargo compile and preview worker; scripted agent"]
fn the_native_shell_runs_a_task_adopts_the_staged_preview_and_undoes_it() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let sdk = PathBuf::from(std::env::var_os("SDK_ACTIVE").expect("SDK_ACTIVE"));
    assert!(
        tool_available("Xvfb") && tool_available("xdotool"),
        "Xvfb and xdotool are required"
    );
    // The shell finds the SDK under $HOME/.fframes/sdk/active.
    let home = sdk
        .ancestors()
        .nth(3)
        .expect("SDK_ACTIVE ends in .fframes/sdk/active")
        .to_path_buf();
    assert!(
        home.join(".fframes/sdk/active").exists(),
        "SDK_ACTIVE layout"
    );
    let manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(sdk.join("compatibility.json")).unwrap(),
    )
    .unwrap();

    let temp = tempfile::Builder::new()
        .prefix("fft-x11")
        .tempdir_in("/tmp")
        .unwrap();
    let project = temp.path().join("video");
    preview_fixture::create(&project, &manifest);
    let data = temp.path().join("data");
    let runtime = temp.path().join("run");
    let evidence = temp.path().join("agent");
    for dir in [
        data.join("fframes-studio"),
        runtime.clone(),
        evidence.clone(),
    ] {
        fs::create_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // The adapter description exactly as the Setup tab saves it. Writer containment is
    // never a setting: the shell below is started with the explicit test-only injection
    // (`--test-writer-containment`), which the panel labels as not a qualification.
    fs::write(
        data.join("fframes-studio/agent-adapter.json"),
        serde_json::json!({
            "provider": "scripted-peer",
            "executable": python3(),
            "args": [
                format!("{}/tests/support/acp-agent.py", env!("CARGO_MANIFEST_DIR")),
                evidence.to_string_lossy()
            ],
            "auth_env_names": [],
            "mcp": "unsupported"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        evidence.join("plan.json"),
        serde_json::json!({"turns": [{
            "text": "Adding a note.",
            "tool": 1,
            "write": {"notes.txt": "note from the scripted peer\n"},
            // A media-changing edit: same timeline, different audio samples, so the
            // video/audio identities and the audio digest must move together.
            "write_b64": {"media/cue.wav": base64(&preview_fixture::wave(880., 330.))}
        }]})
        .to_string(),
    )
    .unwrap();

    let screen = Screen::start();
    let telemetry = Telemetry(temp.path().join("telemetry.json"));
    let log = fs::File::create(temp.path().join("studio.log")).unwrap();
    let _app = spawn(
        Command::new(env!("CARGO_BIN_EXE_fframes-studio"))
            .args(["qualify-m3", "--project"])
            .arg(&project)
            .arg("--telemetry")
            .arg(&telemetry.0)
            .args(["--test-writer-containment", "x11-scripted-peer"])
            .env("DISPLAY", &screen.display)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .stdout(log.try_clone().unwrap())
            .stderr(log),
    );

    // Opening a project opens the workflow and launches no agent.
    let opened = telemetry.wait("the workflow to open", |v| {
        v["agent_workflow_open"] == true && v["sdk_ready_for_agent"] == true
    });
    assert_eq!(phase(&opened), "", "no task yet");
    assert_eq!(opened["panel"]["owned_processes"], 0);
    assert!(
        !evidence.join("starts").exists(),
        "no agent was started on open"
    );

    // Baseline playback: build the project's current source with the shell's own button,
    // then move the playhead with its frame-step button so the hand-off has a seek intent
    // to carry (serial > 0, position 3) and a known video/audio identity to replace.
    let window = screen.window();
    let original_audio = fs::read(project.join("media/cue.wav")).unwrap();
    let ready = telemetry.settled("the build button", |v| {
        v["buttons"]["build-preview"].clone()
    });
    screen.click(&window, button_centre(&ready, "build-preview"));
    telemetry.wait("the baseline preview to display", |v| {
        displayed(v).is_object() && v["preview"]["status"] == "displayed"
    });
    for step in 1..=3 {
        let now = telemetry.read().unwrap();
        screen.click(&window, button_centre(&now, "next-frame"));
        telemetry.wait("the frame step", |v| v["preview"]["position"] == step);
    }
    let before = telemetry.wait("the stepped frame to display", |v| {
        v["preview"]["painted_frame"] == 3
            && v["preview"]["presented_serial"] == v["preview"]["seek_serial"]
    });
    let base = displayed(&before).clone();
    assert_eq!(base["video_revision"], base["audio_revision"], "{before}");
    let base_epoch = before["preview"]["transport_epoch"].as_u64().unwrap();
    let base_serial = before["preview"]["seek_serial"].as_u64().unwrap();
    assert!(base_serial >= 3, "{before}");

    // Type a brief into the native prompt and send it with Enter.
    click_prompt(&screen, &window, &telemetry);
    screen.xdotool(&[
        "type",
        "--window",
        &window,
        "--clearmodifiers",
        "add a note file",
    ]);
    screen.xdotool(&["key", "Return"]);

    let accepted = telemetry.wait("acceptance", |v| phase(v) == "Accepted");
    // Every typed character, spaces included, reached the agent: Space in the prompt is
    // text, never a transport shortcut.
    let prompts = fs::read_to_string(evidence.join("prompts.jsonl")).unwrap();
    assert!(prompts.contains("add a note file"), "{prompts}");
    assert_eq!(accepted["panel"]["repair_used"], 0);
    // The staged preview was adopted on the UI thread and installed with matching
    // video/audio under a fresh epoch: the displayed revision is the accepted one and
    // the awaiting-preview label is gone.
    let shown = telemetry.wait("the matching preview to display", |v| {
        v["panel"]["handoff"] == "Displayed"
            && v["awaiting_preview"] == false
            && v["displayed_revision"].is_string()
            && displayed(v)["video_revision"] != base["video_revision"]
    });
    let published = shown["displayed_revision"].as_str().unwrap().to_owned();
    assert_eq!(shown["panel"]["undo"], "available");
    assert!(
        project.join("notes.txt").is_file(),
        "the accepted edit is in the project"
    );
    let applied = displayed(&shown);
    // Matching identity: the installed video, the installed audio and the shell's
    // displayed revision are one revision, and it is the accepted one.
    assert_eq!(
        applied["video_revision"], applied["audio_revision"],
        "{shown}"
    );
    assert_eq!(
        applied["video_revision"].as_str().unwrap(),
        published,
        "{shown}"
    );
    // The edit changed the audio, and the installed mix is the new one.
    assert_ne!(applied["audio_sha256"], base["audio_sha256"], "{shown}");
    assert_ne!(
        fs::read(project.join("media/cue.wav")).unwrap(),
        original_audio
    );
    // A fresh audio epoch, and the playhead intent survived the hand-off: same position,
    // video frame and PCM window start for it, under a serial that did not go backwards.
    let applied_epoch = shown["preview"]["transport_epoch"].as_u64().unwrap();
    assert!(
        applied_epoch > base_epoch,
        "{base_epoch} -> {applied_epoch}: {shown}"
    );
    assert_eq!(shown["preview"]["position"], 3, "{shown}");
    assert_eq!(applied["frame_index"], 3, "{shown}");
    assert_eq!(shown["preview"]["painted_frame"], 3, "{shown}");
    assert_eq!(applied["position"], 3, "{shown}");
    assert_eq!(
        applied["pcm_start_sample"].as_u64().unwrap(),
        3 * applied["sample_rate"].as_u64().unwrap() / applied["fps"].as_u64().unwrap(),
        "{shown}"
    );
    assert!(shown["preview"]["seek_serial"].as_u64().unwrap() >= base_serial);
    assert_eq!(
        applied["frame_serial"], shown["preview"]["seek_serial"],
        "{shown}"
    );

    // Undo from the Project tab: validated, published, handed off again.
    screen.click(&window, PROJECT_TAB);
    std::thread::sleep(Duration::from_millis(500));
    screen.click(&window, UNDO_BUTTON);
    let undone = telemetry.wait("the Undo hand-off", |v| {
        v["panel"]["undo"] == "unavailable"
            && v["panel"]["handoff"] == "Displayed"
            && v["displayed_revision"].is_string()
            && v["displayed_revision"].as_str() != Some(published.as_str())
            && v["awaiting_preview"] == false
    });
    assert!(
        !project.join("notes.txt").exists(),
        "Undo removed the file the edit added"
    );
    assert_eq!(
        fs::read(project.join("media/cue.wav")).unwrap(),
        original_audio,
        "Undo restored the original audio"
    );
    // Undo's hand-off is again a matching video/audio install under yet another epoch,
    // the original mix is back, and the playhead intent is still carried.
    let reverted = displayed(&undone);
    assert_eq!(
        reverted["video_revision"], reverted["audio_revision"],
        "{undone}"
    );
    assert_ne!(
        reverted["video_revision"].as_str().unwrap(),
        published,
        "{undone}"
    );
    assert_eq!(reverted["audio_sha256"], base["audio_sha256"], "{undone}");
    assert!(
        undone["preview"]["transport_epoch"].as_u64().unwrap() > applied_epoch,
        "{undone}"
    );
    assert_eq!(undone["preview"]["position"], 3, "{undone}");
    assert_eq!(reverted["frame_index"], 3, "{undone}");
    assert_eq!(undone["preview"]["painted_frame"], 3, "{undone}");
    // Only the worker of the displayed preview is still owned: no adapter, no tool worker.
    // The task's evidence workers are released when it ends; their teardown is
    // asynchronous, so wait for it instead of sampling one instant.
    telemetry.wait("only the displayed worker to remain owned", |v| {
        v["panel"]["owned_processes"].as_u64().unwrap_or(0) <= 1
    });
    // The scripted peer was started for the one task (Undo launches no agent).
    let starts = fs::read_to_string(evidence.join("starts")).unwrap_or_default();
    assert_eq!(starts.lines().count(), 1, "{starts:?}");
}

impl Screen {
    /// A PNG of the whole X screen (xwd, converted by ffmpeg) into a qualification evidence
    /// directory when configured; otherwise nothing is captured.
    fn screenshot(&self, name: &str) {
        let directory = std::env::var_os("M5_NATIVE_EVIDENCE")
            .or_else(|| std::env::var_os("M4_NATIVE_EVIDENCE"))
            .map(PathBuf::from);
        let Some(dir) = directory else {
            return;
        };
        fs::create_dir_all(&dir).unwrap();
        let raw = dir.join(format!("{name}.xwd"));
        let capture = Command::new("xwd")
            .env("DISPLAY", &self.display)
            .args(["-root", "-silent", "-out"])
            .arg(&raw)
            .status()
            .expect("xwd");
        assert!(capture.success(), "xwd -root");
        let convert = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&raw)
            .arg(dir.join(format!("{name}.png")))
            .status()
            .expect("ffmpeg");
        assert!(convert.success(), "ffmpeg xwd -> png");
        let _ = fs::remove_file(raw);
    }
}

fn scope_of(value: &Value) -> Option<&str> {
    value["panel"]["task_scope"].as_str()
}

/// Scene scope and range scope through the real shell: the timeline's own keys select the
/// scope, the conversation panel names it before submit, the submitted task freezes it,
/// and the before evidence is rendered from the frozen base before the candidate's after
/// evidence is rendered from the validated candidate. The agent is the scripted peer.
#[test]
#[ignore = "requires SDK_ACTIVE, Xvfb and xdotool; real Cargo compile and preview worker; scripted agent"]
fn the_native_shell_freezes_scene_and_range_scope_and_shows_before_and_after_evidence() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let sdk = PathBuf::from(std::env::var_os("SDK_ACTIVE").expect("SDK_ACTIVE"));
    assert!(
        tool_available("Xvfb") && tool_available("xdotool"),
        "Xvfb and xdotool are required"
    );
    let home = sdk
        .ancestors()
        .nth(3)
        .expect("SDK_ACTIVE ends in .fframes/sdk/active")
        .to_path_buf();
    let manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(sdk.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    let temp = tempfile::Builder::new()
        .prefix("fft-x11s")
        .tempdir_in("/tmp")
        .unwrap();
    let project = temp.path().join("video");
    preview_fixture::create(&project, &manifest);
    let data = temp.path().join("data");
    let runtime = temp.path().join("run");
    let evidence = temp.path().join("agent");
    for dir in [
        data.join("fframes-studio"),
        runtime.clone(),
        evidence.clone(),
    ] {
        fs::create_dir_all(dir).unwrap();
    }
    // ACP children use a deliberately restricted environment; negotiate image support
    // through the fixture root rather than inheriting this test process's environment.
    fs::write(evidence.join("prompt-image"), "enabled\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(
        data.join("fframes-studio/agent-adapter.json"),
        serde_json::json!({
            "provider": "scripted-peer",
            "executable": python3(),
            "args": [
                format!("{}/tests/support/acp-agent.py", env!("CARGO_MANIFEST_DIR")),
                evidence.to_string_lossy()
            ],
            "auth_env_names": [],
            "mcp": "unsupported"
        })
        .to_string(),
    )
    .unwrap();
    // Two tasks; the second edit changes nothing the first one did not.
    fs::write(
        evidence.join("plan.json"),
        serde_json::json!({"turns": [
            {"text": "Adding a note.", "tool": 1, "write": {"notes.txt": "scene note\n"}},
            {"text": "Adding another note.", "tool": 1, "write": {"notes2.txt": "range note\n"}},
            {"text": "Holding.", "wait_file": "go", "tool": 1, "write": {"notes3.txt": "held note\n"}}
        ]})
        .to_string(),
    )
    .unwrap();

    let screen = Screen::start();
    let telemetry = Telemetry(temp.path().join("telemetry.json"));
    let log = fs::File::create(temp.path().join("studio.log")).unwrap();
    let _app = spawn(
        Command::new(env!("CARGO_BIN_EXE_fframes-studio"))
            .args(["qualify-m3", "--project"])
            .arg(&project)
            .arg("--telemetry")
            .arg(&telemetry.0)
            .args(["--test-writer-containment", "x11-scripted-peer"])
            .env("DISPLAY", &screen.display)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .stdout(log.try_clone().unwrap())
            .stderr(log),
    );
    telemetry.wait("the workflow to open", |v| {
        v["agent_workflow_open"] == true && v["sdk_ready_for_agent"] == true
    });
    let window = screen.window();
    let ready = telemetry.settled("the build button", |v| {
        v["buttons"]["build-preview"].clone()
    });
    screen.click(&window, button_centre(&ready, "build-preview"));
    let built = telemetry.wait("the baseline preview to display", |v| {
        displayed(v).is_object() && v["preview"]["status"] == "displayed"
    });
    let base_revision = built["displayed_revision"].as_str().unwrap().to_owned();

    // Whole project is the default scope, named before any task exists.
    let whole = telemetry.wait("the submit scope to be named", |v| {
        v["panel"]["submit_scope"].is_string()
    });
    assert!(
        whole["panel"]["submit_scope"]
            .as_str()
            .unwrap()
            .starts_with("Whole project"),
        "{whole}"
    );

    // Focus the timeline by seeking on its ruler, then `s` selects the scene at the
    // playhead through the timeline's own key handler.
    let laid_out = telemetry.settled("the timeline ruler", |v| v["ruler_bounds"].clone());
    let ruler = laid_out["ruler_bounds"].as_array().unwrap();
    let x = ruler[0].as_f64().unwrap() + ruler[2].as_f64().unwrap() * 0.1;
    let y = ruler[1].as_f64().unwrap() + 10.;
    screen.click(&window, (x.round() as u32, y.round() as u32));
    screen.xdotool(&["key", "--window", &window, "s"]);
    let scene = telemetry.wait("the scene scope to be named", |v| {
        v["panel"]["submit_scope"]
            .as_str()
            .is_some_and(|s| s.starts_with("Scene "))
    });
    let scene_label = scene["panel"]["submit_scope"].as_str().unwrap().to_owned();
    screen.screenshot("scene-selected-before-submit");
    // Validated changes wait for the user: the review shows before and after evidence.
    screen.click(&window, PROJECT_TAB);
    let policy = telemetry.settled("the review-policy button", |v| {
        v["panel"]["buttons"]["agent-policy"].clone()
    });
    screen.click(&window, button_centre(&policy["panel"], "agent-policy"));
    telemetry.wait("manual review to be selected", |v| {
        v["panel"]["review_policy"] == "ManualReview"
    });
    screen.click(&window, CHAT_TAB);

    click_prompt(&screen, &window, &telemetry);
    screen.xdotool(&[
        "type",
        "--window",
        &window,
        "--clearmodifiers",
        "tighten this scene",
    ]);
    screen.xdotool(&["key", "Return"]);

    // The submitted task froze exactly the scene the panel named, and its before
    // evidence is rendered from the displayed (frozen base) revision.
    let frozen = telemetry.wait("the frozen scene scope with before evidence", |v| {
        scope_of(v) == Some(scene_label.as_str())
            && v["panel"]["before_evidence"]["state"] == "Ready"
    });
    let before = &frozen["panel"]["before_evidence"];
    assert!(
        base_revision.starts_with(before["revision"].as_str().unwrap()),
        "before evidence must be rendered from the displayed revision: {frozen}"
    );
    assert!(
        before["artifacts"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{frozen}"
    );
    screen.screenshot("scene-task-frozen-before-evidence");
    let review = telemetry.wait("the candidate to await review with after evidence", |v| {
        phase(v) == "Waiting for your review" && v["panel"]["after_evidence"]["state"] == "Ready"
    });
    let after = &review["panel"]["after_evidence"];
    assert_ne!(
        after["revision"], before["revision"],
        "the candidate is a different revision than the frozen base: {review}"
    );
    let frames_of = |view: &Value| -> Vec<(Value, Value)> {
        view["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["label"].clone(), a["frames"].clone()))
            .collect()
    };
    assert_eq!(
        frames_of(after),
        frames_of(before),
        "after evidence covers the same frames as before: {review}"
    );
    let expected_thumbnails = before["artifacts"].as_array().unwrap().len()
        + after["artifacts"].as_array().unwrap().len();
    telemetry.wait("before/after evidence thumbnails to load", |v| {
        v["panel"]["evidence_preview_count"]
            .as_u64()
            .is_some_and(|count| count as usize >= expected_thumbnails)
    });
    assert!(
        !project.join("notes.txt").exists(),
        "nothing is applied before review"
    );
    screen.screenshot("scene-task-review-before-and-after");
    let review = telemetry.settled("the Apply button", |v| {
        v["panel"]["buttons"]["agent-apply"].clone()
    });
    screen.click(&window, button_centre(&review["panel"], "agent-apply"));
    telemetry.wait("the scene task acceptance", |v| phase(v) == "Accepted");
    assert!(project.join("notes.txt").is_file());
    telemetry.wait("the accepted preview to display", |v| {
        v["awaiting_preview"] == false
            && v["panel"]["handoff"] == "Displayed"
            && v["displayed_revision"].as_str() != Some(base_revision.as_str())
    });

    // The range task uses the default AutoApply policy. Its terminal task snapshot must
    // already have both evidence images, not merely artifact references that were just
    // released during finalization.
    screen.click(&window, PROJECT_TAB);
    let policy = telemetry.settled("the review-policy button", |v| {
        v["panel"]["buttons"]["agent-policy"].clone()
    });
    screen.click(&window, button_centre(&policy["panel"], "agent-policy"));
    telemetry.wait("AutoApply to be selected", |v| {
        v["panel"]["review_policy"] == "AutoApply"
    });
    screen.click(&window, CHAT_TAB);

    // A frame range from the playhead with the timeline's own bracket keys. The layout
    // moved with the hand-off: measure the ruler again.
    let laid_out = telemetry.settled("the timeline ruler", |v| v["ruler_bounds"].clone());
    let ruler = laid_out["ruler_bounds"].as_array().unwrap();
    let x = ruler[0].as_f64().unwrap() + ruler[2].as_f64().unwrap() * 0.1;
    let y = ruler[1].as_f64().unwrap() + 10.;
    screen.click(&window, (x.round() as u32, y.round() as u32));
    screen.xdotool(&["key", "--window", &window, "bracketleft"]);
    let end_x = ruler[0].as_f64().unwrap() + ruler[2].as_f64().unwrap() * 0.2;
    screen.click(&window, (end_x.round() as u32, y.round() as u32));
    screen.xdotool(&["key", "--window", &window, "bracketright"]);
    let range = telemetry.wait("the non-empty range scope to be named", |v| {
        let selected = &v["timeline_selection"]["range"];
        selected[0].as_u64() < selected[1].as_u64()
            && v["panel"]["submit_scope"]
                .as_str()
                .is_some_and(|s| s.starts_with("Frames ["))
    });
    let range_label = range["panel"]["submit_scope"].as_str().unwrap().to_owned();
    screen.screenshot("range-selected-before-submit");
    click_prompt(&screen, &window, &telemetry);
    screen.xdotool(&[
        "type",
        "--window",
        &window,
        "--clearmodifiers",
        "retime this range",
    ]);
    screen.xdotool(&["key", "Return"]);
    let frozen = telemetry.wait("the frozen range scope with before evidence", |v| {
        scope_of(v) == Some(range_label.as_str())
            && v["panel"]["before_evidence"]["state"] == "Ready"
    });
    assert_eq!(frozen["panel"]["task_scope"], range_label, "{frozen}");
    screen.screenshot("range-task-frozen-before-evidence");
    telemetry.wait("the AutoApply range candidate evidence", |v| {
        scope_of(v) == Some(range_label.as_str())
            && v["panel"]["after_evidence"]["state"] == "Ready"
    });
    let range_evidence = telemetry.read().expect("range evidence telemetry");
    let range_before = &range_evidence["panel"]["before_evidence"];
    let range_after = &range_evidence["panel"]["after_evidence"];
    assert_ne!(
        range_before["revision"], range_after["revision"],
        "AutoApply evidence names the frozen base and candidate revisions"
    );
    assert_eq!(
        frames_of(range_before),
        frames_of(range_after),
        "AutoApply evidence covers the same selected scope frames"
    );
    let expected_thumbnails = range_before["artifacts"].as_array().unwrap().len()
        + range_after["artifacts"].as_array().unwrap().len();
    let accepted = telemetry.wait("AutoApply terminal evidence thumbnails", |v| {
        phase(v) == "Accepted"
            && v["panel"]["after_evidence"]["state"] == "Released"
            && v["panel"]["evidence_preview_count"]
                .as_u64()
                .is_some_and(|count| count as usize >= expected_thumbnails)
    });
    assert!(
        accepted["panel"]["before_evidence"]["artifacts"].is_array()
            && accepted["panel"]["after_evidence"]["artifacts"].is_array(),
        "the terminal AutoApply task retains both evidence references: {accepted}"
    );
    screen.screenshot("range-task-review-before-and-after");
    assert!(project.join("notes2.txt").is_file());

    // The stale-scope case needs a candidate that waits for an explicit review action.
    screen.click(&window, PROJECT_TAB);
    let policy = telemetry.settled("the review-policy button", |v| {
        v["panel"]["buttons"]["agent-policy"].clone()
    });
    screen.click(&window, button_centre(&policy["panel"], "agent-policy"));
    telemetry.wait("ManualReview to be selected", |v| {
        v["panel"]["review_policy"] == "ManualReview"
    });
    screen.click(&window, CHAT_TAB);

    // Stale queued scope: a held task runs, a scoped brief queues behind it, the displayed
    // preview is rebuilt (a new identity), and the queued scope is marked stale and then
    // refused when its turn comes. It is never retargeted and never starts an agent.
    telemetry.wait("the accepted range preview to display", |v| {
        v["awaiting_preview"] == false && v["panel"]["handoff"] == "Displayed"
    });
    let laid_out = telemetry.settled("the timeline ruler", |v| v["ruler_bounds"].clone());
    let ruler = laid_out["ruler_bounds"].as_array().unwrap();
    let x = ruler[0].as_f64().unwrap() + ruler[2].as_f64().unwrap() * 0.1;
    let y = ruler[1].as_f64().unwrap() + 10.;
    screen.click(&window, (x.round() as u32, y.round() as u32));
    screen.xdotool(&["key", "--window", &window, "s"]);
    telemetry.wait("a scene scope for the held task", |v| {
        v["panel"]["submit_scope"]
            .as_str()
            .is_some_and(|s| s.starts_with("Scene "))
    });
    click_prompt(&screen, &window, &telemetry);
    screen.xdotool(&[
        "type",
        "--window",
        &window,
        "--clearmodifiers",
        "hold this scene",
    ]);
    screen.xdotool(&["key", "Return"]);
    telemetry.wait("the held task to start", |v| {
        v["panel"]["task_scope"]
            .as_str()
            .is_some_and(|s| s.starts_with("Scene "))
            && phase(v) != "Accepted"
            && phase(v) != "Waiting for your review"
    });
    click_prompt(&screen, &window, &telemetry);
    screen.xdotool(&[
        "type",
        "--window",
        &window,
        "--clearmodifiers",
        "queued scoped brief",
    ]);
    screen.xdotool(&["key", "Return"]);
    telemetry.wait("the scoped brief to queue", |v| v["panel"]["queue"] == 1);
    let build = telemetry.settled("the build button", |v| {
        v["buttons"]["build-preview"].clone()
    });
    screen.click(&window, button_centre(&build, "build-preview"));
    let stale = telemetry.wait("the queued scope to be marked stale", |v| {
        v["panel"]["queue_stale"] == 1
    });
    screen.screenshot("stale-queued-scope");
    assert_eq!(stale["panel"]["queue"], 1, "{stale}");
    fs::write(evidence.join("go"), "go").unwrap();
    telemetry.wait("the held task to await review", |v| {
        phase(v) == "Waiting for your review"
    });
    let review = telemetry.settled("the Apply button", |v| {
        v["panel"]["buttons"]["agent-apply"].clone()
    });
    screen.click(&window, button_centre(&review["panel"], "agent-apply"));
    telemetry.wait("the stale scope to be refused", |v| {
        v["panel"]["queue"] == 0
            && v["panel"]["error_codes"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c == "stale_task_scope"))
    });
    let refused = telemetry.settled("the stale-scope refusal panel", |v| {
        serde_json::json!({
            "queue": v["panel"]["queue"],
            "error_codes": v["panel"]["error_codes"],
        })
    });
    assert_eq!(refused["panel"]["queue"], 0, "{refused}");
    assert!(
        refused["panel"]["error_codes"]
            .as_array()
            .is_some_and(|codes| codes.iter().any(|code| code == "stale_task_scope")),
        "{refused}"
    );
    screen.screenshot("stale-queued-scope-refused");
    let prompts = fs::read_to_string(evidence.join("prompts.jsonl")).unwrap();
    assert_eq!(
        prompts.lines().count(),
        3,
        "no agent prompt for the stale brief: {refused}"
    );
    assert!(!prompts.contains("queued scoped brief"), "{prompts}");
    for prompt in prompts
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
    {
        let text = prompt["text"].as_str().expect("recorded text prompt");
        assert!(
            text.contains("sha256"),
            "the text identifies the evidence: {prompt}"
        );
        assert!(
            !text.contains("base64") && !text.contains("data:image"),
            "image data is never embedded in prompt text: {prompt}"
        );
        let images = prompt["image_blocks"].as_array().expect("image metadata");
        assert_eq!(
            images.len(),
            text.matches("(app artifact art-").count(),
            "before selected/boundary images are attached: {prompt}"
        );
        assert!(
            images.iter().all(|image| {
                image["mime_type"] == "image/png"
                    && image["data_length"].as_u64().is_some_and(|n| n > 0)
                    && image["decoded_bytes"].as_u64().is_some_and(|n| n > 0)
            }),
            "only bounded PNG image blocks are sent: {prompt}"
        );
        let total_bytes: u64 = images
            .iter()
            .map(|image| image["decoded_bytes"].as_u64().unwrap())
            .sum();
        assert!(
            total_bytes <= 4 * 1024 * 1024,
            "image prompt budget: {prompt}"
        );
    }
}

/// The actual generated starter title is selected from the displayed managed-worker frame.
/// The exact title rectangle is taken from frame metadata and mapped through the shell's
/// measured image transform, so this test fails rather than passing by clicking a guessed
/// location. It also exercises keyboard cycling, pointer-centred zoom, pan and rectangle
/// fallback through native X11 input.
#[test]
#[ignore = "requires SDK_BUNDLE, Xvfb, xdotool, xwd and ffmpeg; native managed-SDK canvas interaction"]
fn the_native_shell_selects_the_managed_starter_title_and_keeps_rectangle_scope_nonsemantic() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        tool_available("Xvfb")
            && tool_available("xdotool")
            && tool_available("xwd")
            && tool_available("ffmpeg"),
        "Xvfb, xdotool, xwd and ffmpeg are required"
    );
    let bundle = PathBuf::from(std::env::var_os("SDK_BUNDLE").expect("SDK_BUNDLE"));
    let manifest = CompatibilityManifest::from_json_str(
        &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("fft-m5-x11")
        .tempdir_in("/tmp")
        .unwrap();
    let artifacts: Vec<_> = manifest
        .artifacts
        .iter()
        .map(|artifact| {
            (
                artifact.clone(),
                bundle.join(artifact.url.trim_start_matches("file://")),
            )
        })
        .collect();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let sdk = SdkInstaller::new(home.join(".fframes/sdk"))
        .install_from_local_artifacts(&manifest, &artifacts)
        .unwrap();
    let project = temporary.path().join("video");
    studio_project::create(
        &project,
        "M5 native canvas selection",
        sdk_pin(&manifest),
        &manifest.fframes_version,
        "0.1.0",
    )
    .unwrap();
    assert!(sdk.join("compatibility.json").is_file());

    let data = std::env::var_os("M5_NATIVE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary.path().join("data"));
    let runtime = temporary.path().join("runtime");
    fs::create_dir_all(data.join("fframes-studio")).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let screen = Screen::start();
    let telemetry = Telemetry(temporary.path().join("telemetry.json"));
    let log = fs::File::create(temporary.path().join("studio.log")).unwrap();
    let _app = spawn(
        Command::new(env!("CARGO_BIN_EXE_fframes-studio"))
            .args(["qualify-m3", "--project"])
            .arg(&project)
            .arg("--telemetry")
            .arg(&telemetry.0)
            .env("DISPLAY", &screen.display)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .stdout(log.try_clone().unwrap())
            .stderr(log),
    );
    let opened = telemetry.wait("the managed-SDK project to open", |value| {
        value["agent_workflow_open"] == true && value["sdk_ready_for_agent"] == true
    });
    assert_eq!(phase(&opened), "");
    let window = screen.window();
    let build = telemetry.settled("the build button", |value| {
        value["buttons"]["build-preview"].clone()
    });
    screen.click(&window, button_centre(&build, "build-preview"));
    let measured = telemetry.wait("managed title metadata", |value| {
        value["canvas"]["metadata_status"] == "Supported"
            && value["canvas"]["objects"]
                .as_array()
                .is_some_and(|objects| {
                    objects.iter().any(|object| {
                        object["identity"]["object_key"] == "headline"
                            && object["bounds"].is_object()
                    })
                })
    });
    assert!(
        measured["canvas"]["rendered_bright_pixels"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "the GPUI image buffer must contain visible title pixels"
    );
    let objects = measured["canvas"]["objects"].as_array().unwrap();
    let title = objects
        .iter()
        .find(|object| {
            object["identity"]["scene_instance_key"] == "starter-video"
                && object["identity"]["component_key"] == "starter-title"
                && object["identity"]["object_key"] == "headline"
                && object["identity"]["repeat_key"] == "primary"
        })
        .expect("registered title is present in the displayed frame metadata");
    let bounds = &title["bounds"];
    let image = &measured["canvas"]["image_bounds"];
    let dimensions = &measured["canvas"]["video_dimensions"];
    let origin = &measured["canvas"]["canvas_origin"];
    let number = |value: &Value| value.as_f64().expect("finite geometry");
    let (video_width, video_height) = (number(&dimensions[0]), number(&dimensions[1]));
    let x = number(&origin[0])
        + number(&image["x"])
        + (number(&bounds["x"]) + number(&bounds["width"]) / 2.) / video_width
            * number(&image["width"]);
    let y = number(&origin[1])
        + number(&image["y"])
        + (number(&bounds["y"]) + number(&bounds["height"]) / 2.) / video_height
            * number(&image["height"]);
    screen.click(&window, (x.round() as u32, y.round() as u32));
    let selected = telemetry.wait("the registered title to be selected", |value| {
        value["canvas"]["selection"]["identity"]["object_key"] == "headline"
    });
    assert_eq!(
        selected["canvas"]["selection"]["identity"], title["identity"],
        "the native hit test must select the exact semantic tuple"
    );
    screen.xdotool(&["key", "--window", &window, "c"]);
    let cycled_to_group = telemetry.wait("canvas selection cycling to the parent group", |value| {
        value["canvas"]["selection"]["identity"]["object_key"] == "root"
    });
    assert_eq!(
        cycled_to_group["canvas"]["selection"]["identity"]["component_key"], "video",
        "overlap cycling exposes the registered parent group"
    );
    screen.xdotool(&["key", "--window", &window, "c"]);
    let cycled = telemetry.wait("canvas cycling back to the title", |value| {
        value["canvas"]["selection"]["identity"] == title["identity"]
    });
    assert_eq!(cycled["canvas"]["selection"]["identity"], title["identity"]);
    screen.screenshot("m5-managed-title-selected");

    let before_zoom = cycled["canvas"]["image_bounds"]["width"].as_f64().unwrap();
    screen.xdotool(&[
        "mousemove",
        "--window",
        &window,
        &x.round().to_string(),
        &y.round().to_string(),
        "keydown",
        "ctrl",
        "click",
        "4",
        "keyup",
        "ctrl",
    ]);
    let zoomed = telemetry.wait("pointer-centred canvas zoom", |value| {
        value["canvas"]["image_bounds"]["width"]
            .as_f64()
            .is_some_and(|width| width > before_zoom * 1.05)
    });
    assert_eq!(
        zoomed["canvas"]["selection"]["identity"], title["identity"],
        "zoom does not rebind the selected title"
    );

    let before_pan_x = zoomed["canvas"]["image_bounds"]["x"].as_f64().unwrap();
    screen.xdotool(&[
        "mousemove",
        "--window",
        &window,
        &x.round().to_string(),
        &y.round().to_string(),
        "mousedown",
        "2",
        "mousemove_relative",
        "--sync",
        "25",
        "15",
        "mouseup",
        "2",
    ]);
    let panned = telemetry.wait("canvas pan", |value| {
        value["canvas"]["image_bounds"]["x"]
            .as_f64()
            .is_some_and(|current| (current - before_pan_x).abs() > 10.)
    });
    assert_eq!(
        panned["canvas"]["selection"]["identity"], title["identity"],
        "pan does not rebind the selected title"
    );

    let image = &panned["canvas"]["image_bounds"];
    let origin = &panned["canvas"]["canvas_origin"];
    let extent = &panned["canvas"]["canvas_extent"];
    let x1 = number(&origin[0]) + number(&image["x"]) + number(&image["width"]) * 0.35;
    let y1 = number(&origin[1]) + number(&image["y"]) + number(&image["height"]) * 0.35;
    let x2 = number(&origin[0])
        + (number(&image["x"]) + number(&image["width"]) * 0.65).min(number(&extent[0]) - 2.);
    let y2 = number(&origin[1])
        + (number(&image["y"]) + number(&image["height"]) * 0.65).min(number(&extent[1]) - 2.);
    screen.xdotool(&["key", "--window", &window, "Escape"]);
    screen.xdotool(&["keydown", "shift"]);
    screen.xdotool(&[
        "mousemove",
        "--window",
        &window,
        &x1.round().to_string(),
        &y1.round().to_string(),
        "mousedown",
        "1",
    ]);
    let drawing = telemetry.wait(
        "rectangle drag to begin inside the painted image",
        |value| value["canvas"]["drawing_rectangle"] == true,
    );
    assert_eq!(drawing["canvas"]["selection"], Value::Null);
    screen.xdotool(&[
        "mousemove",
        "--window",
        &window,
        &x2.round().to_string(),
        &y2.round().to_string(),
        "mousemove_relative",
        "--sync",
        "1",
        "0",
    ]);
    let preview_rectangle = telemetry.wait("rectangle drag to update its video bounds", |value| {
        value["canvas"]["rectangle"].is_object() && value["canvas"]["selection"].is_null()
    });
    assert!(
        preview_rectangle["canvas"]["rectangle"]["width"]
            .as_f64()
            .unwrap()
            > 0.
    );
    assert!(
        preview_rectangle["canvas"]["rectangle"]["height"]
            .as_f64()
            .unwrap()
            > 0.
    );
    screen.xdotool(&["mouseup", "1"]);
    screen.xdotool(&["keyup", "shift"]);
    let scope_deadline = Instant::now() + Duration::from_secs(15);
    let rectangle = loop {
        if let Some(value) = telemetry.read()
            && value["canvas"]["rectangle"].is_object()
            && value["canvas"]["selection"].is_null()
            && value["panel"]["submit_scope"]
                .as_str()
                .is_some_and(|scope| scope.starts_with("Rectangle "))
        {
            break value;
        }
        assert!(
            Instant::now() < scope_deadline,
            "rectangle scope was not presented as the submission scope; last telemetry: {:?}",
            telemetry.read()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(rectangle["canvas"]["metadata_status"], "Supported");
    assert!(rectangle["canvas"]["rectangle"]["width"].as_f64().unwrap() > 0.);
    assert!(rectangle["canvas"]["rectangle"]["height"].as_f64().unwrap() > 0.);
    screen.screenshot("m5-nonsemantic-rectangle-scope");
}
