//! Spike probe: drives the project's real `studio_worker` process through Hello,
//! Timeline, ScaledFrame and SetTiming without restarting it, then checks that the
//! render path given the same timing document draws the same frames.
use fframes_studio_protocol::*;
use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

const SCALE: f64 = 0.5;

fn send(stdin: &mut impl Write, request: &PreviewRequest) {
    let bytes = serde_json::to_vec(request).unwrap();
    stdin.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
    stdin.write_all(&bytes).unwrap();
    stdin.flush().unwrap();
}
fn read_framed(stream: &mut impl Read) -> Vec<u8> {
    let mut len = [0; 4];
    stream.read_exact(&mut len).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut bytes).unwrap();
    bytes
}
fn reply(stdout: &mut impl Read) -> PreviewResponse {
    serde_json::from_slice(&read_framed(stdout)).unwrap()
}

/// (scene key by band color, scene frame by bar width) of an RGBA frame at SCALE.
fn decode(rgba: &[u8], width: usize) -> (&'static str, usize) {
    let px = |x: usize, y: usize| {
        let i = (y * width + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    let band = px(10, (700. * SCALE) as usize);
    let scene = match band {
        [r, _, _] if r > 180 && band[1] < 100 => "intro",
        [_, g, _] if g > 180 => "body",
        [_, _, b] if b > 180 => "outro",
        _ => "?",
    };
    let y = (880. * SCALE) as usize;
    let white = (0..width).take_while(|&x| px(x, y)[0] > 200 && px(x, y)[2] > 200).count();
    // bar = (index + 1) * 60 px at full size
    let index = ((white as f64 / SCALE) / 60.).round() as usize - 1;
    (scene, index)
}

fn main() {
    let root = std::env::current_dir().unwrap();
    let worker = std::env::current_exe().unwrap().with_file_name("studio_worker.exe");
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let cache = root.join("target").join("probe-audio-cache");
    std::fs::create_dir_all(&cache).unwrap();
    let mut child = Command::new(&worker)
        .args(["--preview-worker", "--generation", "1", "--revision", "rev", "--frame-port"])
        .arg(port.to_string())
        .args(["--project-id", "probe", "--open-session", "s2", "--sdk-version", "spike"])
        .arg("--audio-cache")
        .arg(&cache)
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (mut bulk, _) = listener.accept().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let pid = child.id();
    let identity = PreviewIdentity {
        project_id: "probe".into(),
        open_session: "s2".into(),
        source_revision: "rev".into(),
        worker_generation: 1,
    };
    let mut id = 0;
    let mut env = || {
        id += 1;
        PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: identity.clone(),
            request_id: id,
        }
    };

    let hello_id = env().request_id;
    send(
        &mut stdin,
        &PreviewRequest::Hello(PreviewHelloRequest {
            offered_versions: vec![PREVIEW_CONTRACT_VERSION],
            required_capabilities: vec![],
            request_id: hello_id,
        }),
    );
    let PreviewResponse::Hello(hello) = reply(&mut stdout) else { panic!() };
    let offered = hello.capabilities.iter().any(|c| c == SET_TIMING_CAPABILITY);
    println!("worker pid {pid}, set_timing_v1 offered: {offered}");
    assert!(offered);

    let mut timeline = |stdin: &mut std::process::ChildStdin, stdout: &mut BufReader<_>, e| {
        send(stdin, &PreviewRequest::Timeline(e));
        let PreviewResponse::Timeline(t) = reply(stdout) else { panic!() };
        let scenes: Vec<_> = t
            .scenes
            .iter()
            .map(|s| format!("{}@{}..{}", s.instance_id, s.start_frame, s.end_frame))
            .collect();
        println!("timeline: {} frames {}", t.total_frames, scenes.join(" "));
        t.total_frames
    };
    let mut frame = |stdin: &mut std::process::ChildStdin,
                     stdout: &mut BufReader<_>,
                     bulk: &mut std::net::TcpStream,
                     e: PreviewEnvelope,
                     index| {
        send(
            stdin,
            &PreviewRequest::ScaledFrame(ScaledFrameRequest {
                envelope: e,
                frame_index: index,
                seek_serial: index as u64,
                scale: SCALE,
            }),
        );
        let PreviewResponse::ScaledFrame(r) = reply(stdout) else { panic!() };
        let _header = read_framed(bulk);
        let mut payload = vec![0; r.record.payload_len];
        bulk.read_exact(&mut payload).unwrap();
        (payload, r.header.width as usize, r.header.height as usize)
    };

    assert_eq!(timeline(&mut stdin, &mut stdout, env()), 90);
    for index in [15, 45, 75] {
        let (rgba, w, _) = frame(&mut stdin, &mut stdout, &mut bulk, env(), index);
        println!("  code order frame {index}: {:?}", decode(&rgba, w));
    }

    // Reorder (outro first), trim (body keeps its first 10 frames, intro loses 10)
    // and split (body plays again from its frame 20).
    let doc = fframes::TimingDoc {
        version: fframes::TIMING_DOC_VERSION,
        clips: [("outro", 0, 0), ("body", 0, 20), ("intro", 10, 0), ("body", 20, 0)]
            .into_iter()
            .map(|(key, s, e)| fframes::TimingClip {
                key: key.into(),
                trim_start_frames: s,
                trim_end_frames: e,
            })
            .collect(),
    };
    send(
        &mut stdin,
        &PreviewRequest::SetTiming(SetTimingRequest {
            envelope: env(),
            timing_json: Some(serde_json::to_string(&doc).unwrap()),
        }),
    );
    let PreviewResponse::TimingApplied(applied) = reply(&mut stdout) else { panic!() };
    println!("SetTiming applied: {applied:?}");
    assert_eq!(timeline(&mut stdin, &mut stdout, env()), 70);
    let expected = [(15, ("outro", 15)), (35, ("body", 5)), (45, ("intro", 15)), (65, ("body", 25))];
    let mut previews = Vec::new();
    for (index, want) in expected {
        let (rgba, w, h) = frame(&mut stdin, &mut stdout, &mut bulk, env(), index);
        let got = decode(&rgba, w);
        println!("  timed frame {index}: {got:?} (expected {want:?})");
        assert_eq!(got, want);
        previews.push((index, rgba, w, h));
    }
    send(&mut stdin, &PreviewRequest::Shutdown(env()));
    let _ = reply(&mut stdout);
    assert!(child.wait().unwrap().success());
    println!("worker {pid} served every request without a restart or rebuild");

    // Render path with the same document.
    let directory = fframes::MediaDirectory::read_folder("media").unwrap();
    let media = directory.process_media_source().unwrap();
    let video = studio_video::StudioVideo::new().unwrap();
    let mut options = fframes::RenderOptions {
        media: Some(&media),
        timing: Some(&doc),
        scale_resolution: SCALE,
        ..Default::default()
    };
    for (index, preview, _, _) in &previews {
        let rendered = fframes::render_frame(
            *index,
            &video,
            fframes::cpu::CpuRenderingBackend::default(),
            &options,
        )
        .unwrap();
        let differing = rendered.iter().zip(preview).filter(|(a, b)| a != b).count();
        println!("  render_frame {index}: {differing} of {} bytes differ from preview", rendered.len());
        assert_eq!(differing, 0);
    }

    options.video_encoder_options = fframes::EncoderOptions {
        codec: Some(fframes::ffmpeg_sys_fframes::AVCodecID::AV_CODEC_ID_MPEG4),
        bitrate: Some(20_000_000),
        ..Default::default()
    };
    let output = root.join("target").join("timing-probe.mp4");
    fframes::render(&output, &video, fframes::cpu::CpuRenderingBackend::default(), &options).unwrap();
    for (index, preview, w, h) in &previews {
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-vf", &format!("select=eq(n\\,{index})"), "-vframes", "1"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(decoded.len(), w * h * 4, "decoded frame {index}");
        let got = decode(&decoded, *w);
        let mean = decoded
            .iter()
            .zip(preview)
            .map(|(a, b)| a.abs_diff(*b) as f64)
            .sum::<f64>()
            / decoded.len() as f64;
        println!("  encoded frame {index}: {got:?}, mean abs diff to preview {mean:.2}");
        assert_eq!(got, decode(preview, *w));
        assert!(mean < 4.0);
    }
    println!("PASS");
}
