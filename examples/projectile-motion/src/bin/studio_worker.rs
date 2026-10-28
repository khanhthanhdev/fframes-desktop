//! The same deck served through Studio's isolated preview/export protocol.
use std::{io, sync::Arc};

use fframes::{MediaDirectory, RenderOptions};
use fframes_studio_kit::PropsStore;
use fframes_studio_protocol::{
    PreviewIdentity,
    export::{EXPORT_CAPABILITIES_FLAG, EXPORT_FLAG},
};
use fframes_studio_runtime::{
    PreviewWorkerConfig, WorkerTransport, print_export_capabilities_with_props,
    run_export_with_props, serve_preview_worker,
};
use projectile_motion::ProjectileMotionVideo;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == EXPORT_CAPABILITIES_FLAG) {
        return print_export_capabilities_with_props::<ProjectileMotionVideo>();
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .ok_or_else(|| format!("Missing {flag}"))
    };
    let directory = MediaDirectory::read_folder("media")?;
    let media = directory.process_media_source()?;
    let props = Arc::new(PropsStore::default());
    props.listen();
    let hooks = props.hooks();
    let video = ProjectileMotionVideo::new(props);
    let options = RenderOptions {
        media: Some(&media),
        ..Default::default()
    };
    if args.iter().any(|arg| arg == EXPORT_FLAG) {
        return run_export_with_props(&video, &options, &args, &hooks);
    }
    let identity = PreviewIdentity {
        project_id: value("--project-id")?.clone(),
        open_session: value("--open-session")?.clone(),
        source_revision: value("--revision")?.clone(),
        worker_generation: value("--generation")?.parse()?,
    };
    let mut config =
        PreviewWorkerConfig::new(identity, value("--sdk-version")?.clone(), fframes::VERSION)
            .with_timing_args(&args)
            .with_props_args(&args)
            .with_props(hooks);
    if let Ok(cache) = value("--audio-cache") {
        config.cache_directory = cache.into();
    }
    let frames =
        std::net::TcpStream::connect(("127.0.0.1", value("--frame-port")?.parse::<u16>()?))?;
    serve_preview_worker(
        &video,
        &options,
        WorkerTransport::new(io::stdin(), io::stdout(), frames),
        config,
    )?;
    Ok(())
}
