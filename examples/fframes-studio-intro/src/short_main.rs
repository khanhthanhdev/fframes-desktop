use fframes::{
    AudioMixOptions, CombinedMediaProvider, EncoderOptions, LimiterOptions, MediaDirectory,
    MediaProvider, RenderOptions, StaticMediaProvider, cli,
};
use fframes_skia_renderer::{
    SkiaFFramesRenderer, SkiaPipelineConcurrencyPolicy, SkiaPipelineConfig,
};
use fframes_studio_intro::{
    StudioIntroMedia,
    short::{HEIGHT, StudioShortVideo, WIDTH},
};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let media = StudioIntroMedia::prepare().expect("media");
    // The soundtrack and sound effects are the ones of `fframes-intro`: the
    // scenes are cut to its beat grid, so the folder is shared, not copied.
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fframes-intro/dynamic_media");
    let dir = MediaDirectory::read_folder(&folder).expect("dynamic media folder");
    let dynamic = dir.process_media_source().expect("dynamic media");
    let all = CombinedMediaProvider::from([&media as &dyn MediaProvider, &dynamic]);
    #[cfg(target_os = "macos")]
    let gpu = fframes_skia_renderer::metal::SkiaMetalCtx::new(WIDTH, HEIGHT).expect("GPU context");
    #[cfg(not(target_os = "macos"))]
    let gpu =
        fframes_skia_renderer::vulkan::SkiaVulkanCtx::new(WIDTH, HEIGHT).expect("GPU context");
    let pipeline = SkiaPipelineConfig {
        concurrency_policy: SkiaPipelineConcurrencyPolicy::MaxPerformance,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    let backend = SkiaFFramesRenderer::new_metal(&gpu, pipeline);
    #[cfg(not(target_os = "macos"))]
    let backend = SkiaFFramesRenderer::new_vulkan(&gpu, pipeline);

    cli::new(
        &StudioShortVideo,
        RenderOptions {
            media: Some(&all),
            video_encoder_options: EncoderOptions {
                preferred_encoder: Some("libx264"),
                codec_params: Some(&[("crf", "16"), ("preset", "medium"), ("tune", "film")]),
                ..Default::default()
            },
            // the music edit measures about -17.5 LUFS; lift it to about -14 for the web
            audio_mix: AudioMixOptions {
                master_gain_db: 4.5,
                limiter: Some(LimiterOptions {
                    ceiling_db: -1.3,
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .default_output("fframes-studio-intro-short-9x16.mp4")
    .backend(backend.expect("skia renderer"))
    .preview(fframes_native_player::cli_preview)
    .run()
}
