//! The one-shot export of a project worker (`--export-v1`) and the capabilities it
//! reports (`--export-capabilities-v1`). See [`fframes_studio_protocol::export`].
use std::error::Error;
use std::path::{Path, PathBuf};

use fframes::ffmpeg_sys_fframes::AVCodecID;
use fframes::media::{has_encoder, probe_streams};
use fframes::{EncoderOptions, RenderOptions, TimingDoc, Video, VideoSize};
use fframes_studio_protocol::export::{
    EXPORT_CAPABILITIES_VERSION, ExportCapabilities, ExportCodec, ExportContainer, ExportFormat,
    ExportSettings,
};

use crate::renderer::{BackendChoice, RendererPreference};

type ExportResult<T> = Result<T, Box<dyn Error>>;

/// Opus encodes only 48 kHz among the rates fframes mixes at.
const OPUS_SAMPLE_RATE: usize = 48_000;

/// What this worker can export: every container and codec whose encoders its FFmpeg has.
pub fn export_capabilities<V: Video>() -> ExportCapabilities {
    let formats = ExportContainer::ALL
        .into_iter()
        .flat_map(|container| {
            ExportCodec::ALL
                .into_iter()
                .filter(move |codec| container.holds(*codec))
                .map(move |codec| ExportFormat { container, codec })
        })
        .filter(|format| {
            video_encoder(format.codec).is_some() && has_encoder(format.container.audio_encoder())
        })
        .collect();
    ExportCapabilities {
        version: EXPORT_CAPABILITIES_VERSION,
        width: V::WIDTH as u32,
        height: V::HEIGHT as u32,
        fps: V::FPS as u32,
        timing: true,
        props: false,
        formats,
    }
}

/// Prints [`export_capabilities`] as one JSON document on stdout.
pub fn print_export_capabilities<V: Video>() -> ExportResult<()> {
    println!("{}", serde_json::to_string(&export_capabilities::<V>())?);
    Ok(())
}

/// [`print_export_capabilities`] of a worker that exports with
/// [`run_export_with_props`](crate::run_export_with_props), which reads `--props-file`.
pub fn print_export_capabilities_with_props<V: Video>() -> ExportResult<()> {
    let capabilities = ExportCapabilities {
        props: true,
        ..export_capabilities::<V>()
    };
    println!("{}", serde_json::to_string(&capabilities)?);
    Ok(())
}

/// Renders `video` to `--output <file>` with the settings in `args` and checks what was
/// written. `options` carries the project media; encoders, size, frame rate and timing
/// come from the arguments. A `--timing-file` that is missing or invalid fails the
/// export instead of silently exporting the code order.
pub fn run_export<V: Video + Send>(
    video: &V,
    options: &RenderOptions<'_, '_>,
    args: &[String],
) -> ExportResult<()> {
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .map(|index| {
                args.get(index + 1)
                    .ok_or_else(|| format!("{flag} needs a value"))
            })
            .transpose()
    };
    let settings = ExportSettings::from_args(args)?;
    settings.validate(V::FPS as u32)?;
    let output = PathBuf::from(value("--output")?.ok_or("Missing --output")?);
    let extension = settings.container.extension();
    if output.extension().and_then(|extension| extension.to_str()) != Some(extension) {
        return Err(format!("export output must be a .{extension} file").into());
    }
    let encoder = video_encoder(settings.codec).ok_or_else(|| {
        format!(
            "this FFmpeg build has no {} encoder",
            settings.codec.stream_codec_name()
        )
    })?;
    let audio_encoder = settings.container.audio_encoder();
    if !has_encoder(audio_encoder) {
        return Err(format!("this FFmpeg build has no {audio_encoder} encoder").into());
    }
    let timing = match value("--timing-file")? {
        None => None,
        Some(path) => Some(
            TimingDoc::read_file(Path::new(path))?
                .ok_or_else(|| format!("timing file {path} does not exist"))?,
        ),
    };

    let size = VideoSize::new_scaled(V::WIDTH, V::HEIGHT, settings.scale);
    let fps = settings.fps.map_or(V::FPS, |fps| fps as usize);
    let mut options = RenderOptions {
        media: options.media,
        logger: fframes::FFramesLoggerVariant::Json,
        scale_resolution: settings.scale,
        override_fps: settings.fps.map(|fps| fps as usize),
        timing,
        ..Default::default()
    };
    options.video_encoder_options = EncoderOptions {
        codec: Some(codec_id(settings.codec)),
        preferred_encoder: Some(encoder),
        bitrate: Some(settings.codec.video_bitrate(
            size.width as u32,
            size.height as u32,
            fps as u32,
        ) as i64),
        codec_params: encoder_params(encoder),
        ..Default::default()
    };
    options.audio_encoder_options = EncoderOptions {
        codec: Some(match settings.container {
            ExportContainer::Mp4 => AVCodecID::AV_CODEC_ID_AAC,
            ExportContainer::Webm => AVCodecID::AV_CODEC_ID_OPUS,
        }),
        preferred_encoder: Some(audio_encoder),
        bitrate: Some(settings.container.audio_bitrate() as i64),
        ..Default::default()
    };
    if settings.container == ExportContainer::Webm {
        options.audio_encoder_options.sample_rate = OPUS_SAMPLE_RATE;
    }
    render_with_selected_backend(
        &output,
        video,
        &options,
        RendererPreference::from_args(args),
        (size.width, size.height, fps),
    )?;
    verify_export(&output, video, &settings, size, fps)
}

/// Renders with the backend `preference` selects at the export size. Skia is used only after
/// it encoded a probe frame; a Skia export that fails in the backend rather than in the video
/// is rendered again once with the built-in CPU renderer.
fn render_with_selected_backend<V: Video + Send>(
    output: &Path,
    video: &V,
    options: &RenderOptions<'_, '_>,
    preference: RendererPreference,
    (width, height, fps): (usize, usize, usize),
) -> ExportResult<()> {
    #[cfg(feature = "skia")]
    {
        use crate::renderer::{CPU_BACKEND, one_line};
        use fframes::{EncoderFrameRenderer, FFramesRenderBackend, VideoEncoderInfo};
        use fframes_skia_renderer::{
            SkiaFFramesRenderer, SkiaPipelineConcurrencyPolicy, SkiaPipelineConfig,
        };

        let cpu = |reason: String| {
            let reason = one_line(&reason);
            eprintln!(
                "{}",
                serde_json::json!({ "event": "warning", "message": reason })
            );
            log_backend(&BackendChoice::new(CPU_BACKEND, Some(reason)));
        };

        if preference != RendererPreference::Cpu && options.frame_stride(V::FPS)? != 1 {
            // the Skia pipeline encodes every frame of the video
            cpu(format!(
                "Skia cannot export at {fps} fps, the video runs at {}; exporting with the CPU \
                 renderer, which skips shader layers",
                V::FPS
            ));
        } else {
            let selection =
                crate::renderer::select(preference, width, height, crate::renderer::platform_gpu)?;
            match &selection.ctx {
                None => log_backend(&selection.choice),
                Some(ctx) => {
                    let pipeline = SkiaPipelineConfig {
                        concurrency_policy: SkiaPipelineConcurrencyPolicy::MaxPerformance,
                        ..Default::default()
                    };
                    let backend = SkiaFFramesRenderer::new(pipeline, ctx);
                    // a missing encoder is not the backend's fault and fails the export
                    let encoder = VideoEncoderInfo::for_output(
                        output,
                        (width as i32, height as i32, fps as i32),
                        &options.video_encoder_options,
                    )
                    .map_err(|error| fframes::FFramesRendererError::from_chunk(0, error))?;
                    match encode_probe(&backend, &encoder, (width as u32, height as u32)) {
                        Err(error) => cpu(format!(
                            "{} could not encode a probe frame, exporting with the CPU renderer: \
                             {error}",
                            selection.choice.name
                        )),
                        Ok(()) => {
                            log_backend(&selection.choice);
                            match fframes::render(output, video, backend, options) {
                                Ok(()) => return Ok(()),
                                Err(error) if is_backend_failure(&error) => cpu(format!(
                                    "{} failed, exporting with the CPU renderer: {error}",
                                    selection.choice.name
                                )),
                                Err(error) => return Err(error.into()),
                            }
                        }
                    }
                }
            }
        }

        /// Opens the encoder input the way the export will and draws one frame into it, so
        /// GPU format conversion and hardware frames are tried before the export starts.
        fn encode_probe(
            backend: &impl FFramesRenderBackend,
            encoder: &VideoEncoderInfo<'_>,
            (width, height): (u32, u32),
        ) -> fframes::FFramesRendererResult<()> {
            let input = backend
                .negotiate_encoder_input(encoder)
                .map_err(|error| fframes::FFramesRendererError::from_chunk(0, error))?;
            let tree = fframes::usvgr::Tree::from_str(
                ENCODE_PROBE_SVG,
                &fframes::usvgr::Options {
                    fast_shapes: true,
                    ..Default::default()
                },
                &Default::default(),
            )
            .map_err(|error| fframes::FFramesRendererError::Internal(error.to_string()))?;
            backend
                .encoder_frame_renderer(&input, width, height)?
                .render_tree(&tree, fframes::Color::BLACK)?;
            Ok(())
        }
    }
    #[cfg(not(feature = "skia"))]
    {
        let _ = (width, height, fps);
        log_backend(&crate::renderer::cpu_only(preference)?);
    }
    fframes::render(
        output,
        video,
        fframes::cpu::CpuRenderingBackend::default(),
        options,
    )?;
    Ok(())
}

/// The frame a Skia export encodes before it is used: a rounded rectangle and a circle.
#[cfg(feature = "skia")]
const ENCODE_PROBE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="32" rx="8" fill="#ff0000"/><circle cx="32" cy="48" r="12" fill="#00ff00"/></svg>"##;

/// One `backend` event on stderr; the export service keeps it with the diagnostics.
fn log_backend(choice: &BackendChoice) {
    eprintln!(
        "{}",
        serde_json::json!({ "event": "backend", "backend": choice.name, "detail": choice.detail })
    );
}

/// Whether a Skia export failed in the backend rather than in the video or the encoder:
/// panics, missing media, encoder errors and cancellation fail the same way on the CPU.
#[cfg(feature = "skia")]
fn is_backend_failure(error: &fframes::FFramesRendererError) -> bool {
    matches!(
        error,
        fframes::FFramesRendererError::Skia(_)
            | fframes::FFramesRendererError::Internal(_)
            | fframes::FFramesRendererError::ConcurrencyError
    )
}

/// The first encoder of `codec` this FFmpeg build has.
fn video_encoder(codec: ExportCodec) -> Option<&'static str> {
    codec
        .encoders()
        .iter()
        .copied()
        .find(|name| has_encoder(name))
}

fn codec_id(codec: ExportCodec) -> AVCodecID {
    match codec {
        ExportCodec::Mpeg4 => AVCodecID::AV_CODEC_ID_MPEG4,
        ExportCodec::H264 => AVCodecID::AV_CODEC_ID_H264,
        ExportCodec::Hevc => AVCodecID::AV_CODEC_ID_HEVC,
        ExportCodec::Vp9 => AVCodecID::AV_CODEC_ID_VP9,
    }
}

/// Speed settings only: no quality target, so the bitrate shared with size estimates applies.
/// OpenH264 takes that bitrate too, but must not skip frames to hold it: exports are
/// frame-exact.
fn encoder_params(encoder: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match encoder {
        "libx264" | "libx265" => Some(&[("preset", "veryfast")]),
        "libopenh264" => Some(&[("allow_skip_frames", "0")]),
        "libvpx-vp9" => Some(&[("deadline", "good"), ("cpu-used", "4"), ("row-mt", "1")]),
        _ => None,
    }
}

/// Checks the written file holds the requested codec, size and frame rate, that its
/// first frame decodes, and that its audio decodes when the video has audio.
fn verify_export<V: Video>(
    output: &Path,
    video: &V,
    settings: &ExportSettings,
    size: VideoSize,
    fps: usize,
) -> ExportResult<()> {
    let streams = probe_streams(output)
        .map_err(|error| format!("could not read the exported file: {error:?}"))?;
    let codec = settings.codec.stream_codec_name();
    if streams.video_codec.as_deref() != Some(codec) {
        return Err(format!(
            "export holds {} video instead of {codec}",
            streams.video_codec.as_deref().unwrap_or("no")
        )
        .into());
    }
    if (streams.width, streams.height) != (size.width as u32, size.height as u32) {
        return Err(format!(
            "export is {}x{} instead of {}x{}",
            streams.width, streams.height, size.width, size.height
        )
        .into());
    }
    if (streams.fps - fps as f64).abs() > 0.01 {
        return Err(format!("export runs at {} fps instead of {fps}", streams.fps).into());
    }

    let mut decoder = unsafe { fframes::media::FFmpegDecoder::new(output, fps, 1) }
        .map_err(|error| format!("could not open exported video: {error:?}"))?;
    if !unsafe { decoder.decode_up_to(0) }
        .map_err(|error| format!("could not decode exported video: {error:?}"))?
    {
        return Err("export contains no decodable video frame".into());
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
            return Err("export audio stream failed sample verification".into());
        }
    }
    Ok(())
}
