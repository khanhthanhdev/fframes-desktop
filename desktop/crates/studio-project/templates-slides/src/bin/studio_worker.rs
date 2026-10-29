use fframes_studio_kit::PropsStore;
use fframes_studio_protocol::PreviewIdentity;
use fframes_studio_protocol::export::{EXPORT_CAPABILITIES_FLAG, EXPORT_FLAG};
use fframes_studio_runtime::{
    PreviewWorkerConfig, WorkerTransport, print_export_capabilities_with_props,
    run_export_with_props, serve_preview_worker, serve_worker,
};
use std::io;
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == EXPORT_CAPABILITIES_FLAG) {
        return Ok(print_export_capabilities_with_props::<studio_video::StudioVideo>()?);
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .ok_or_else(|| format!("Missing {flag}"))
    };
    let directory = fframes::MediaDirectory::read_folder("media")
        .map_err(|error| format!("could not read project media: {error:?}"))?;
    let media = directory
        .process_media_source()
        .map_err(|error| format!("could not prepare project media: {error:?}"))?;
    // The Props panel's documents reach the slides through this store.
    let props = Arc::new(PropsStore::default());
    // Records each editable slide text's schema and values for the Props panel.
    props.listen();
    let hooks = props.hooks();
    let video = studio_video::StudioVideo::new(props)?;
    let options = fframes::RenderOptions {
        media: Some(&media),
        ..Default::default()
    };
    // One-shot export: the app passes the output, the settings, the timing and props documents.
    if args.iter().any(|arg| arg == EXPORT_FLAG) {
        return run_export_with_props(&video, &options, &args, &hooks);
    }
    let generation = value("--generation")?.parse()?;
    let revision = value("--revision")?;
    let port: u16 = value("--frame-port")?.parse()?;
    let frames = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let transport = WorkerTransport::new(io::stdin(), io::stdout(), frames);
    if args.iter().any(|arg| arg == "--preview-worker") {
        let identity = PreviewIdentity {
            project_id: value("--project-id")?.clone(),
            open_session: value("--open-session")?.clone(),
            source_revision: revision.clone(),
            worker_generation: generation,
        };
        let mut config =
            PreviewWorkerConfig::new(identity, value("--sdk-version")?.clone(), fframes::VERSION)
                .with_timing_args(&args)
                .with_props_args(&args)
                .with_props(hooks);
        if let Ok(cache) = value("--audio-cache") {
            config.cache_directory = cache.into();
        }
        serve_preview_worker(&video, &options, transport, config)?;
    } else {
        serve_worker(&video, &options, &[], transport, revision, generation)?;
    }
    Ok(())
}
