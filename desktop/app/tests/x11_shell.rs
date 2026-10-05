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
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[path = "support/preview_fixture.rs"]
#[allow(dead_code)]
mod preview_fixture;

const WAIT: Duration = Duration::from_secs(240);

/// Window-relative positions in the 1280x800 layout.
const PROMPT: (u32, u32) = (1078, 715);
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

fn displayed(value: &Value) -> &Value {
    &value["preview"]["displayed"]
}

fn phase(value: &Value) -> &str {
    value["panel"]["phase"].as_str().unwrap_or("")
}

#[test]
#[ignore = "requires SDK_ACTIVE, Xvfb and xdotool; real Cargo compile and preview worker; scripted agent"]
fn the_native_shell_runs_a_task_adopts_the_staged_preview_and_undoes_it() {
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
    let ready = telemetry.wait("the build button to be measured", |v| {
        v["buttons"]["build-preview"].is_array()
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
    screen.click(&window, PROMPT);
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
    assert!(
        undone["panel"]["owned_processes"].as_u64().unwrap_or(0) <= 1,
        "{undone}"
    );
    // The scripted peer was started for the one task (Undo launches no agent).
    let starts = fs::read_to_string(evidence.join("starts")).unwrap_or_default();
    assert_eq!(starts.lines().count(), 1, "{starts:?}");
}
