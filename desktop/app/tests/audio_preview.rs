use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};

use fframes_studio::{
    audio_service::{AudioEvent, AudioService, OutputDevice},
    frame_image,
    preview_coordinator::{BuildSpec, PreviewCoordinator},
    worker_project::{acquire_build_lock, launch_preview_worker},
};
use studio_bootstrap::{ProcessTreeManager, SpawnOptions};
use studio_engine::{Controller, JobKind, app_paths::AppPaths};
use studio_sdk::ProjectManager;

#[path = "support/preview_fixture.rs"]
mod preview_fixture;

fn decode_wav(bytes: &[u8]) -> (u32, Vec<[f32; 2]>) {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut at = 12;
    let mut format = None;
    let mut data = None;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        match &bytes[at..at + 4] {
            b"fmt " => format = Some(&bytes[body..body + size]),
            b"data" => data = Some(&bytes[body..body + size]),
            _ => {}
        }
        at = body + size + (size & 1);
    }
    let format = format.expect("WAV fmt chunk");
    let encoding = u16::from_le_bytes(format[0..2].try_into().unwrap());
    assert_eq!(u16::from_le_bytes(format[2..4].try_into().unwrap()), 2);
    let rate = u32::from_le_bytes(format[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes(format[14..16].try_into().unwrap());
    let width = usize::from(bits / 8);
    let samples = data
        .expect("WAV data chunk")
        .chunks_exact(width * 2)
        .map(|frame| {
            let sample = |offset| match (encoding, bits) {
                (1, 16) => {
                    f32::from(i16::from_le_bytes(
                        frame[offset..offset + 2].try_into().unwrap(),
                    )) / 32768.
                }
                (3, 32) => f32::from_le_bytes(frame[offset..offset + 4].try_into().unwrap()),
                _ => panic!("unsupported WAV format {encoding}/{bits}"),
            };
            [sample(0), sample(width)]
        })
        .collect();
    (rate, samples)
}

fn run_audio_cli(
    build: &studio_engine::build_materialization::MaterializedBuild,
    output: &std::path::Path,
    manager: &ProcessTreeManager,
) {
    let mut options = SpawnOptions::new("cargo");
    options.args(["run", "--", "audio", "render", "-o"]);
    options.arg(output);
    options.current_dir(&build.root);
    options.env = build.environment.build_child_environment();
    options.stdout = Stdio::piped();
    options.stderr = Stdio::piped();
    let child = manager.spawn(options).unwrap();
    let (mut stdout, mut stderr) = {
        let mut child = child.lock();
        (
            child.child_mut().stdout.take().unwrap(),
            child.child_mut().stderr.take().unwrap(),
        )
    };
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let status = child.lock().wait().unwrap();
    let stdout = out.join().unwrap();
    let stderr = err.join().unwrap();
    assert!(
        status.success(),
        "audio CLI failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
}

fn sdk(temp: &tempfile::TempDir) -> (PathBuf, studio_sdk::CompatibilityManifest) {
    let path = if let Some(bundle) = std::env::var_os("SDK_BUNDLE") {
        let bundle = PathBuf::from(bundle);
        let manifest = studio_sdk::CompatibilityManifest::from_json_str(
            &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
        )
        .unwrap();
        let artifacts: Vec<_> = manifest
            .artifacts
            .iter()
            .map(|a| (a.clone(), bundle.join(a.url.trim_start_matches("file://"))))
            .collect();
        let home = std::env::var_os("AUDIO_PREVIEW_SDK_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| temp.path().join("sdk"));
        studio_sdk::SdkInstaller::new(home)
            .install_from_local_artifacts(&manifest, &artifacts)
            .unwrap()
    } else {
        PathBuf::from(std::env::var_os("SDK_ACTIVE").expect("SDK_ACTIVE or SDK_BUNDLE"))
    };
    let manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(path.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    (path, manifest)
}

#[test]
fn generated_wav_scaffold_is_independently_decodable() {
    let temp = tempfile::tempdir().unwrap();
    let manifest = studio_sdk::CompatibilityManifest::default_linux_x64();
    let root = temp.path().join("generated-wav");
    preview_fixture::create(&root, &manifest);
    assert!(root.join("src/main.rs").is_file(), "ordinary CLI scaffold");
    let (rate, samples) = decode_wav(&fs::read(root.join("media/cue.wav")).unwrap());
    assert_eq!(rate, 48_000);
    assert_eq!(samples.len(), 48_000);
    assert_ne!(samples[120][0], samples[120][1]);
}

fn prepare_real() -> (
    tempfile::TempDir,
    PreviewCoordinator,
    Arc<studio_engine::ReadyPreview>,
    Controller,
) {
    let temp = tempfile::tempdir().unwrap();
    let (sdk, manifest) = sdk(&temp);
    let root = std::env::var_os("AUDIO_PREVIEW_FIXTURE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| temp.path().join("portable-video"));
    preview_fixture::create(&root, &manifest);
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let coordinator = PreviewCoordinator::new(controller.processes.sub_manager());
    let tag = controller.begin_job(JobKind::Build).unwrap();
    coordinator.build(BuildSpec {
        project: controller.project.clone(),
        sdk,
        compatibility: manifest,
        builds: paths.builds(),
        tag,
        compiler: controller.operation_processes(),
        worker: controller.processes.sub_manager(),
    });
    let deadline = Instant::now() + Duration::from_secs(240);
    let ready = loop {
        let event = coordinator.events();
        assert!(event.error.is_none(), "preview failure: {:?}", event.error);
        if let Some(ready) = event.ready {
            break ready;
        }
        assert!(Instant::now() < deadline, "real preview deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    (temp, coordinator, ready, controller)
}

#[test]
#[ignore = "requires SDK_ACTIVE or SDK_BUNDLE; real SDK frame/audio parity qualification"]
fn real_sdk_frame_audio_and_retained_source() {
    let (_temp, coordinator, ready, mut controller) = prepare_real();
    assert_eq!(ready.timeline.total_frames, 165);
    assert_eq!(
        ready
            .timeline
            .scenes
            .iter()
            .map(|s| (s.start_frame, s.end_frame))
            .collect::<Vec<_>>(),
        [(0, 60), (45, 135), (105, 165)]
    );
    assert_eq!(ready.timeline.audio_tracks[0].start_seconds, 5513. / 44100.);
    assert_eq!(ready.audio.sample_rate, 48_000);
    assert_eq!(ready.audio.byte_count, ready.audio.sample_count * 8);
    assert_eq!(ready.audio.sha256.len(), 64);
    let source = ready
        .audio_source
        .as_ref()
        .expect("retained validated PCM")
        .clone();
    let generated = fs::read(source.build().root.join("media/cue.wav")).unwrap();
    let (generated_rate, generated_samples) = decode_wav(&generated);
    assert_eq!(generated_rate, preview_fixture::WAVE_RATE);
    assert_eq!(
        generated_samples.len(),
        preview_fixture::WAVE_FRAMES as usize
    );
    assert_ne!(generated_samples[120][0], generated_samples[120][1]);

    let frame = ready.frame.as_ref().expect("first CPU frame");
    assert_eq!(
        frame.response.header.alpha_mode,
        fframes_studio_protocol::AlphaMode::Straight
    );
    for &(x, y, expected) in &preview_fixture::SWATCH_CENTERS {
        let p = (y as usize * frame.response.header.stride_bytes as usize) + x as usize * 4;
        assert_eq!(&frame.pixels[p..p + 4], &expected, "raw swatch ({x},{y})");
    }
    let bgra =
        frame_image::convert_rgba_to_gpui_bgra(&frame.response.header, &frame.pixels).unwrap();
    let (_, _, blue) = preview_fixture::SWATCH_CENTERS[2];
    let p = blue[3] as usize; // also prevents accidentally treating expected alpha as opaque
    assert_eq!(p, 128);
    let (x, y, _) = preview_fixture::SWATCH_CENTERS[2];
    let i = (y as usize * frame.response.header.width as usize + x as usize) * 4;
    assert_eq!(&bgra[i..i + 4], &[255, 0, 0, 128]);

    let build = source.build().clone();
    let cli_processes = controller.processes.sub_manager();
    let _lock = acquire_build_lock(
        &build.environment.target_dir.join(".build_lock"),
        &cli_processes,
        Duration::from_secs(120),
    )
    .unwrap();
    ProjectManager::build_project(&build.root, &build.environment, &cli_processes).unwrap();
    let png_dir = build.isolated_bin_dir.join("cli-frame");
    let (_, png) =
        ProjectManager::render_frame(&build.root, &build.environment, 0, &png_dir, &cli_processes)
            .unwrap();
    let cli_rgba = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(
        cli_rgba.dimensions(),
        (frame.response.header.width, frame.response.header.height)
    );
    assert_eq!(
        cli_rgba.as_raw(),
        &frame.pixels,
        "ordinary CLI and preview RGBA"
    );

    let worker_processes = controller.processes.sub_manager();
    let mut worker =
        launch_preview_worker(build.clone(), ready.identity().clone(), &worker_processes).unwrap();
    let hello = worker.negotiate().unwrap();
    assert!(
        hello
            .capability_gaps
            .iter()
            .any(|gap| gap == "shader_preview")
    );
    let timeline = worker.timeline().unwrap();
    let inspection = worker.inspect(vec![0]).unwrap();
    assert!(
        inspection
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity
                != fframes_studio_protocol::DiagnosticSeverity::Error),
        "real renderer inspection: {:?}",
        inspection.diagnostics
    );
    assert_eq!(timeline.total_frames, 165);
    // A header/serial match alone cannot detect stale dynamic pixels. Render a
    // distant frame after a thumbnail-sized render and compare its moving box.
    worker.frame(8, 1, 0.125).unwrap();
    let moved = worker.frame(137, 2, 1.).unwrap();
    let pixel = |x: usize, y: usize| {
        let at = y * moved.response.header.stride_bytes as usize + x * 4;
        &moved.pixels[at..at + 4]
    };
    assert_eq!(pixel(1089, 490), &[49, 190, 147, 255]);
    assert_eq!(pixel(130, 490), &[13, 17, 23, 255]);
    let (_, png) = ProjectManager::render_frame(
        &build.root,
        &build.environment,
        137,
        &png_dir,
        &cli_processes,
    )
    .unwrap();
    assert_eq!(
        image::load_from_memory(&png).unwrap().into_rgba8().as_raw(),
        &moved.pixels,
        "nonzero CLI and preview pixels after thumbnail scale"
    );
    let later = worker.frame(19, 3, 1.).unwrap();
    let at = 490 * later.response.header.stride_bytes as usize + 263 * 4;
    assert_eq!(
        &later.pixels[at..at + 4],
        &[49, 190, 147, 255],
        "same-size preview must update dynamic geometry"
    );
    let prepared = worker.prepare_audio(44_100).unwrap();
    let prepared_source = worker.retain_audio_source(&prepared, || false).unwrap();
    let mut prepared_bytes = vec![0; prepared.byte_count as usize];
    use std::io::Seek;
    let mut file = prepared_source.file.try_clone().unwrap();
    file.rewind().unwrap();
    file.read_exact(&mut prepared_bytes).unwrap();
    let prepared_samples: Vec<[f32; 2]> = prepared_bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|b| {
            [
                f32::from_le_bytes(b[..4].try_into().unwrap()),
                f32::from_le_bytes(b[4..].try_into().unwrap()),
            ]
        })
        .collect();
    let wav_path = build.isolated_bin_dir.join("ordinary-cli-44100.wav");
    run_audio_cli(&build, &wav_path, &cli_processes);
    let (cli_rate, cli_samples) = decode_wav(&fs::read(&wav_path).unwrap());
    assert_eq!(cli_rate, 44_100);
    assert_eq!(cli_samples.len(), prepared_samples.len());
    let onset = |samples: &[[f32; 2]]| {
        samples
            .iter()
            .position(|s| s[0].abs() > 1. / 32768. || s[1].abs() > 1. / 32768.)
            .unwrap()
    };
    // The independently specified placement is sample 5513; cue.wav itself starts at zero,
    // so the first non-zero sample is one frame later.
    let prepared_onset = onset(&prepared_samples);
    let cli_onset = onset(&cli_samples);
    assert_eq!(prepared_onset, 5514);
    assert!(
        cli_onset.abs_diff(prepared_onset) <= 1,
        "quantized onset {cli_onset}"
    );
    let mut max_diff = 0.0_f32;
    let mut sum_diff = 0.0_f64;
    for (actual, expected) in cli_samples.iter().zip(&prepared_samples) {
        for channel in 0..2 {
            let difference = (actual[channel] - expected[channel]).abs();
            max_diff = max_diff.max(difference);
            sum_diff += f64::from(difference);
        }
    }
    let mean_diff = sum_diff / (prepared_samples.len() * 2) as f64;
    assert!(
        max_diff <= 2. / 32768. + f32::EPSILON,
        "16-bit WAV quantization plus one-LSB dither: {max_diff}"
    );
    assert!(
        mean_diff <= 1. / 65536.,
        "mean WAV quantization: {mean_diff}"
    );
    assert_eq!(source.descriptor.sample_rate, 48_000);
    assert_eq!(source.descriptor.sample_count, ready.audio.sample_count);
    assert_eq!(source.descriptor.sha256, ready.audio.sha256);

    coordinator.close();
    drop(coordinator);
    // Positioned reads remain valid after worker retirement because the source owns its build.
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        let mut bytes = [0; 32];
        assert_eq!(
            source.file.read_at(&mut bytes, 8 * 7000).unwrap(),
            bytes.len()
        );
        assert!(bytes.iter().any(|b| *b != 0));
    }
    controller.close().unwrap();
    if let Some(path) = std::env::var_os("AUDIO_PREVIEW_EVIDENCE") {
        fs::write(
            path,
            serde_json::json!({
                "frames": 165, "scenes": [[0,60],[45,135],[105,165]], "onset_44100": 5513,
                "prepared_44100_frames": prepared_samples.len(), "max_difference": max_diff,
                "mean_difference": mean_diff, "host_rate": ready.audio.sample_rate,
                "host_samples": ready.audio.sample_count, "host_bytes": ready.audio.byte_count,
                "host_sha256": ready.audio.sha256, "alpha": 128, "shader_gap": true
            })
            .to_string(),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "requires SDK plus an actual CPAL default/virtual output device"]
fn real_cpal_output_clock_mute_pause_seek_and_cleanup() {
    let (_temp, coordinator, ready, mut controller) = prepare_real();
    let service = AudioService::new(Instant::now());
    service.stage(ready.clone(), 1, 0, OutputDevice::Default);
    let deadline = Instant::now() + Duration::from_secs(10);
    let output = loop {
        match service.event() {
            Some(AudioEvent::Staged {
                output: Some(output),
                ..
            }) => break output,
            Some(AudioEvent::Staged {
                unavailable: Some(reason),
                ..
            }) => panic!("no actual CPAL output device: {reason}"),
            Some(AudioEvent::Failed { error, .. }) => panic!("CPAL output failed: {error}"),
            Some(AudioEvent::Lost { .. }) => panic!("CPAL output lost while staging"),
            _ => assert!(Instant::now() < deadline, "CPAL staging deadline"),
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    service.commit(1, true);
    let deadline = Instant::now() + Duration::from_secs(2);
    while output.metrics().submitted_frames == 0 {
        assert!(Instant::now() < deadline, "output cursor start deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
    service.mute(true);
    let before = output.metrics();
    let deadline = Instant::now() + Duration::from_secs(2);
    let muted = loop {
        let metrics = output.metrics();
        if metrics.submitted_frames > before.submitted_frames {
            break metrics;
        }
        assert!(Instant::now() < deadline, "muted cursor deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        muted.submitted_frames > before.submitted_frames,
        "mute must consume clock"
    );
    assert!(output.snapshot().is_some());
    service.commit(1, false);
    std::thread::sleep(Duration::from_millis(40));
    let paused = output.metrics().submitted_frames;
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(
        output.metrics().submitted_frames,
        paused,
        "pause must stop the submitted cursor"
    );

    service.stage(ready.clone(), 2, 30, OutputDevice::Default);
    service.stage(ready.clone(), 3, 90, OutputDevice::Default);
    let deadline = Instant::now() + Duration::from_secs(10);
    let seeked = loop {
        match service.event() {
            Some(AudioEvent::Staged {
                epoch: 3,
                output: Some(output),
                ..
            }) => break output,
            Some(AudioEvent::Staged { epoch: 2, .. }) => {
                panic!("superseded audio stage was installed")
            }
            Some(AudioEvent::Failed { error, .. }) => panic!("seek output failed: {error}"),
            _ => assert!(Instant::now() < deadline, "seek staging deadline"),
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    service.commit(3, true);
    let deadline = Instant::now() + Duration::from_secs(2);
    let snapshot = loop {
        if let Some(snapshot) = seeked.snapshot() {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "seek clock snapshot deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(snapshot.epoch, 3);
    assert!(
        snapshot.start_sample < u64::from(seeked.sample_rate) / 4,
        "new epoch clock must be relative to its seek: {snapshot:?}"
    );

    service.stage(ready.clone(), 4, 90, OutputDevice::Unavailable);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match service.event() {
            Some(AudioEvent::Staged {
                epoch: 4,
                output: None,
                unavailable: None,
                ..
            }) => break,
            Some(AudioEvent::Failed { error, .. }) => panic!("fallback staging failed: {error}"),
            _ => assert!(Instant::now() < deadline, "explicit fallback deadline"),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    service.stop(5);
    // A bad owned artifact is a preparation failure, even with no previous stream.
    // It must never be mistaken for an unavailable device and installed silently.
    let source = ready.audio_source.as_ref().unwrap();
    let path = source
        .build()
        .isolated_bin_dir
        .join("audio")
        .join(format!("{}.pcm", ready.audio.artifact_id));
    use std::io::Write;
    let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.write_all(&f32::NAN.to_le_bytes()).unwrap();
    service.stage(ready, 6, 0, OutputDevice::Default);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match service.event() {
            Some(AudioEvent::Failed { epoch: 6, error }) => {
                assert!(error.contains("nonfinite"));
                break;
            }
            Some(AudioEvent::Staged { epoch: 6, .. }) => {
                panic!("corrupt PCM cannot fall back silently")
            }
            _ => assert!(Instant::now() < deadline, "invalid PCM staging deadline"),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    service.join();
    coordinator.close();
    drop(coordinator);
    controller.close().unwrap();
}
