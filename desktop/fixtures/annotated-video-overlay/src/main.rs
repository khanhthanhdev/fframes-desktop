use fframes::{Color, Duration, Frame, RenderOptions, Svgr, Video, cli};
use fframes_studio_protocol::{PreviewIdentity, Rect};
use fframes_studio_runtime::{
    ElementRegistration, PreviewWorkerConfig, WorkerTransport, serve_preview_worker, serve_worker,
};
use sha2::{Digest, Sha256};
use std::io;

const SOURCE_CODE: &str = include_str!("main.rs");

#[cfg(test)]
mod tests {
    #[test]
    fn anchor_identifies_the_marked_title_in_the_compiled_source() {
        let (hash, start, end) = super::compute_anchor();
        assert_eq!(hash.len(), 64);
        let snippet = &super::SOURCE_CODE[start..end];
        assert!(snippet.contains("let title_text ="));
        assert!(!snippet.contains("ANCHOR_END"));
    }
}

fn compute_anchor() -> (String, usize, usize) {
    let mut hasher = Sha256::new();
    hasher.update(SOURCE_CODE.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    // Match whole marker lines so their string literals are not mistaken for anchors.
    let start_marker = ["/* ANCHOR_START:", " intro.title */"].concat();
    let end_marker = ["/* ANCHOR_END:", " intro.title */"].concat();
    let mut offset = 0;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for line in SOURCE_CODE.split_inclusive('\n') {
        if line.trim() == start_marker {
            starts.push(offset + line.len());
        }
        if line.trim() == end_marker {
            ends.push(offset);
        }
        offset += line.len();
    }
    assert!(
        starts.len() == 1 && ends.len() == 1 && starts[0] < ends[0],
        "source anchor must be unambiguous"
    );
    let (start, end) = (starts[0], ends[0]);

    (hash, start, end)
}
pub struct AnnotatedVideo;

impl Video for AnnotatedVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(5.0)
    }

    fn audio(&self) -> fframes::AudioMap<'_> {
        fframes::AudioMap::none()
    }

    fn render_frame<'a>(
        &'a self,
        _frame: Frame,
        _ctx: &fframes::FFramesContext<'a, '_>,
    ) -> Svgr<'a> {
        /* ANCHOR_START: intro.title */
        let title_text = "fframes desktop studio";
        /* ANCHOR_END: intro.title */

        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width={Self::WIDTH} height={Self::HEIGHT}>
                <rect x="0" y="0" width={Self::WIDTH} height={Self::HEIGHT} fill="#0d1117" />
                <text id="intro.title" x="100" y="300" font-family="DM Sans" font-size="120" fill="#ffffff">
                    {title_text}
                </text>
            </svg>
        )
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let directory = match fframes::MediaDirectory::read_folder("media") {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("Bundled media missing: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let media = match directory.process_media_source() {
        Ok(media) => media,
        Err(error) => {
            eprintln!("Media preparation failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if args.iter().any(|a| a == "--worker") {
        let worker_generation = args
            .iter()
            .position(|a| a == "--generation")
            .and_then(|p| args.get(p + 1))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(1);
        let rev = args
            .iter()
            .position(|a| a == "--revision")
            .and_then(|p| args.get(p + 1))
            .map(|s| s.as_str())
            .unwrap_or("rev_overlay");

        let video = AnnotatedVideo;
        let options = RenderOptions {
            media: Some(&media),
            ..Default::default()
        };
        let (source_hash, byte_start, byte_end) = compute_anchor();
        let registrations = [ElementRegistration {
            source_revision: rev.into(),
            scene_instance_id: "scene_0".into(),
            element_id: "intro.title".into(),
            instance_key: "k_title".into(),
            bounds: Rect {
                x: 100.0,
                y: 180.0,
                width: 1200.0,
                height: 150.0,
            },
            paint_order: 10,
            source_path: "src/main.rs".into(),
            containing_symbol: "render_frame".into(),
            source_hash,
            byte_start,
            byte_end,
        }];

        let frame_out: Box<dyn io::Write + Send> =
            if let Some(pos) = args.iter().position(|a| a == "--frame-port") {
                if let Some(port_str) = args.get(pos + 1) {
                    if let Ok(port) = port_str.parse::<u16>() {
                        let stream = std::net::TcpStream::connect(("127.0.0.1", port))
                            .expect("connect to client frame port");
                        Box::new(stream)
                    } else {
                        Box::new(io::stderr())
                    }
                } else {
                    Box::new(io::stderr())
                }
            } else {
                Box::new(io::stderr())
            };

        let transport = WorkerTransport::new(io::stdin(), io::stdout(), frame_out);
        let result = if args.iter().any(|a| a == "--preview-worker") {
            let value = |flag: &str, fallback: &str| {
                args.iter().position(|a| a == flag).and_then(|p| args.get(p + 1)).map_or_else(|| fallback.to_owned(), Clone::clone)
            };
            let identity = PreviewIdentity { project_id: value("--project-id", "annotated-video"), open_session: value("--open-session", "qualification"), source_revision: rev.into(), worker_generation };
            let mut config = PreviewWorkerConfig::new(identity, value("--sdk-version", "standalone"), "1.1.0");
            if args.iter().any(|a| a == "--audio-cache") { config.cache_directory = value("--audio-cache", "").into(); }
            serve_preview_worker(&video, &options, transport, config)
        } else {
            serve_worker(&video, &options, &registrations, transport, rev, worker_generation)
        };
        if let Err(err) = result {
            eprintln!("Worker error: {err}");
            return std::process::ExitCode::FAILURE;
        }
        std::process::ExitCode::SUCCESS
    } else {
        let video = AnnotatedVideo;
        cli::new(
            &video,
            RenderOptions {
                media: Some(&media),
                ..Default::default()
            },
        )
        .run()
    }
}
