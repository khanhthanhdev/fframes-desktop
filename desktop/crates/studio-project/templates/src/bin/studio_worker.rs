use std::io;
use fframes_studio_runtime::{WorkerTransport, serve_worker};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).ok_or_else(|| format!("Missing {flag}"));
    let generation = value("--generation")?.parse()?;
    let revision = value("--revision")?;
    let port: u16 = value("--frame-port")?.parse()?;
    let frames = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let directory = fframes::MediaDirectory::read_folder("media")?;
    let media = directory.process_media_source()?;
    serve_worker(&studio_video::StudioVideo, &fframes::RenderOptions { media: Some(&media), ..Default::default() }, &[], WorkerTransport::new(io::stdin(), io::stdout(), frames), revision, generation)?;
    Ok(())
}
