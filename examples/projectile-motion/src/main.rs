use std::{path::Path, process::ExitCode, sync::Arc};

use fframes::{MediaDirectory, RenderOptions, cli};
use fframes_skia_renderer::cli::SkiaRenderers;
use fframes_studio_kit::PropsStore;
use projectile_motion::ProjectileMotionVideo;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Projectile motion: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = MediaDirectory::read_folder(root.join("media"))?;
    let media = directory.process_media_source()?;
    let props = Arc::new(PropsStore::load(root.join("props.json"))?);
    let video = ProjectileMotionVideo::new(props);
    Ok(cli::new(
        &video,
        RenderOptions {
            media: Some(&media),
            ..Default::default()
        },
    )
    .timing_file(root.join("timeline.json"))
    .renderers(SkiaRenderers::default())
    .preview(fframes_native_player::cli_preview)
    .default_output("projectile-motion.mp4")
    .run())
}
