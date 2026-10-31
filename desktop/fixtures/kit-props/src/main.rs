//! A Studio worker whose video draws an fframes-kit component marked editable: the props
//! panel's documents reach it through `PropsHooks`, and exports read `--props-file`.
use std::{io, sync::Arc};

use fframes::RenderOptions;
use fframes_kit::prelude::*;
use fframes_studio_kit::PropsStore;
use fframes_studio_protocol::PreviewIdentity;
use fframes_studio_protocol::export::{EXPORT_CAPABILITIES_FLAG, EXPORT_FLAG};
use fframes_studio_runtime::{
    PreviewWorkerConfig, WorkerTransport, print_export_capabilities_with_props,
    run_export_with_props, serve_preview_worker,
};

#[derive(Debug)]
pub struct KitProps {
    theme: Theme,
    store: Arc<PropsStore>,
}

impl Video for KitProps {
    const FPS: usize = 30;
    const WIDTH: usize = 640;
    const HEIGHT: usize = 360;

    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(2.0)
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let mut ui = Ui::new(frame, ctx, &self.theme).with_props(self.store.view());
        let dot = ui!(<Circle editable="dot" cx={320.0} cy={180.0} radius={40.0} />);
        svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360" width="640" height="360">
                <rect x="0" y="0" width="640" height="360" fill="#000000" />
                {dot}
            </svg>
        )
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == EXPORT_CAPABILITIES_FLAG) {
        return Ok(print_export_capabilities_with_props::<KitProps>()?);
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .ok_or_else(|| format!("Missing {flag}"))
    };
    let store = Arc::new(PropsStore::default());
    // Records each editable object's schema and values for the props panel.
    store.listen();
    let hooks = store.hooks();
    let video = KitProps {
        theme: Theme::default(),
        store,
    };
    let options = RenderOptions::default();
    if args.iter().any(|arg| arg == EXPORT_FLAG) {
        return run_export_with_props(&video, &options, &args, &hooks);
    }
    let port: u16 = value("--frame-port")?.parse()?;
    let frames = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let transport = WorkerTransport::new(io::stdin(), io::stdout(), frames);
    let identity = PreviewIdentity {
        project_id: value("--project-id")?.clone(),
        open_session: value("--open-session")?.clone(),
        source_revision: value("--revision")?.clone(),
        worker_generation: value("--generation")?.parse()?,
    };
    let mut config = PreviewWorkerConfig::new(identity, "standalone", fframes::VERSION)
        .with_props_args(&args)
        .with_props(hooks);
    if let Ok(cache) = value("--audio-cache") {
        config.cache_directory = cache.into();
    }
    serve_preview_worker(&video, &options, transport, config)?;
    Ok(())
}
