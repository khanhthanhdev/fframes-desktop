use fframes_studio_protocol::PreviewIdentity;
use fframes_studio_runtime::{
    PreviewWorkerConfig, WorkerTransport, serve_preview_worker, serve_worker,
};
use std::io;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--export-v1") {
        return run_export(&args);
    }
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .ok_or_else(|| format!("Missing {flag}"))
    };
    let generation = value("--generation")?.parse()?;
    let revision = value("--revision")?;
    let port: u16 = value("--frame-port")?.parse()?;
    let frames = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let directory = fframes::MediaDirectory::read_folder("media")
        .map_err(|error| format!("could not read project media: {error:?}"))?;
    let media = directory
        .process_media_source()
        .map_err(|error| format!("could not prepare project media: {error:?}"))?;
    let video = studio_video::StudioVideo::new()?;
    let transport = WorkerTransport::new(io::stdin(), io::stdout(), frames);
    let options = fframes::RenderOptions {
        media: Some(&media),
        ..Default::default()
    };
    if args.iter().any(|arg| arg == "--preview-worker") {
        let identity = PreviewIdentity {
            project_id: value("--project-id")?.clone(),
            open_session: value("--open-session")?.clone(),
            source_revision: revision.clone(),
            worker_generation: generation,
        };
        let mut config =
            PreviewWorkerConfig::new(identity, value("--sdk-version")?.clone(), "1.2.0");
        if let Ok(cache) = value("--audio-cache") {
            config.cache_directory = cache.into();
        }
        serve_preview_worker(&video, &options, transport, config)?;
    } else {
        serve_worker(&video, &options, &[], transport, revision, generation)?;
    }
    Ok(())
}

/// Additive, one-shot export entry. The app passes an owned temporary output path;
/// the ordinary preview worker protocol and its arguments remain unchanged.
fn run_export(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .ok_or_else(|| format!("Missing {flag}"))
    };
    let output = std::path::PathBuf::from(value("--output")?);
    if output.extension().and_then(|extension| extension.to_str()) != Some("mp4") {
        return Err("export output must be an .mp4 file".into());
    }
    let directory = fframes::MediaDirectory::read_folder("media")
        .map_err(|error| format!("could not read export media: {error:?}"))?;
    let media = directory
        .process_media_source()
        .map_err(|error| format!("could not prepare export media: {error:?}"))?;
    let video = studio_video::StudioVideo::new()?;
    let mut options = fframes::RenderOptions {
        media: Some(&media),
        logger: fframes::FFramesLoggerVariant::Json,
        ..Default::default()
    };
    options.video_encoder_options = fframes::EncoderOptions {
        codec: Some(fframes::ffmpeg_sys_fframes::AVCodecID::AV_CODEC_ID_MPEG4),
        bitrate: Some(8_000_000),
        ..Default::default()
    };
    options.audio_encoder_options = fframes::EncoderOptions {
        codec: Some(fframes::ffmpeg_sys_fframes::AVCodecID::AV_CODEC_ID_AAC),
        bitrate: Some(192_000),
        ..Default::default()
    };
    fframes::render(
        &output,
        &video,
        fframes::cpu::CpuRenderingBackend::default(),
        &options,
    )?;
    verify_export(&output, &video)?;
    Ok(())
}

fn verify_export(
    output: &std::path::Path,
    video: &studio_video::StudioVideo,
) -> Result<(), Box<dyn std::error::Error>> {
    use fframes::Video;

    let mut decoder =
        unsafe { fframes::media::FFmpegDecoder::new(output, studio_video::StudioVideo::FPS, 1) }
            .map_err(|error| format!("could not open exported video: {error:?}"))?;
    if !unsafe { decoder.decode_up_to(0) }
        .map_err(|error| format!("could not decode exported video: {error:?}"))?
    {
        return Err("export MP4 contains no decodable video frame".into());
    }
    let decoded = unsafe {
        decoder
            .get_raw_frame()
            .convert_last_decoded_frame_into_svg_image(None)
    }
    .map_err(|error| format!("could not materialize exported video frame: {error:?}"))?;
    if decoded.width != studio_video::StudioVideo::WIDTH as u32
        || decoded.height != studio_video::StudioVideo::HEIGHT as u32
    {
        return Err("export MP4 dimensions do not match the project timeline".into());
    }

    if video.audio().0.is_some_and(|tracks| !tracks.is_empty()) {
        let mut audio = fframes::media::AudioDecoder::new(output, None)
            .map_err(|error| format!("could not open exported audio: {error:?}"))?;
        let (_, channels) = audio
            .decode_preview_samples(4096)
            .map_err(|error| format!("could not decode exported audio: {error:?}"))?;
        if channels.is_empty()
            || channels
                .iter()
                .any(|channel| channel.iter().any(|s| !s.is_finite()))
        {
            return Err("export MP4 audio stream failed sample verification".into());
        }
    }
    Ok(())
}
