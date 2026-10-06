use std::io;
use fframes_studio_protocol::PreviewIdentity;
use fframes_studio_runtime::{PreviewWorkerConfig, WorkerTransport, serve_preview_worker, serve_worker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).ok_or_else(|| format!("Missing {flag}"));
    let generation = value("--generation")?.parse()?;
    let revision = value("--revision")?;
    let port: u16 = value("--frame-port")?.parse()?;
    let frames = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let directory = fframes::MediaDirectory::read_folder("media")?;
    let media = directory.process_media_source()?;
    let video = studio_video::StudioVideo::new()?;
    let transport = WorkerTransport::new(io::stdin(), io::stdout(), frames);
    let options = fframes::RenderOptions { media: Some(&media), ..Default::default() };
    if args.iter().any(|arg| arg == "--preview-worker") {
        let identity = PreviewIdentity { project_id: value("--project-id")?.clone(), open_session: value("--open-session")?.clone(), source_revision: revision.clone(), worker_generation: generation };
        let mut config = PreviewWorkerConfig::new(identity, value("--sdk-version")?.clone(), "1.1.0");
        if let Ok(cache) = value("--audio-cache") { config.cache_directory = cache.into(); }
        serve_preview_worker(&video, &options, transport, config)?;
    } else {
        serve_worker(&video, &options, &[], transport, revision, generation)?;
    }
    Ok(())
}
