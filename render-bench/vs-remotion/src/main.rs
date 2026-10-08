//! Fixed scene benchmark: wall-clock time to render a complete H.264 MP4.
use std::{path::Path, time::Instant};

use fframes::{
    AudioMap, Color, Duration, EncoderOptions, FFramesContext, FFramesLoggerVariant,
    FFramesRenderBackend, Frame, RenderOptions, StaticMediaProvider, Svgr, Transform, Video,
    cpu::CpuRenderingBackend,
};
use fframes_skia_renderer::{
    SkiaBackend, SkiaCacheConfig, SkiaFFramesRenderer, SkiaPipelineConcurrencyPolicy,
    SkiaPipelineConfig,
};

fframes::include_media_dir!(struct BenchMedia, "render-bench/vs-remotion/media");

const VIDEO_WIDTH: usize = 3840;
const VIDEO_HEIGHT: usize = 2160;
const CIRCLES: usize = 99_000;
const TEXTS: usize = 1000;
const NODES: usize = CIRCLES + TEXTS;
const PANELS: usize = 20;
const PER_PANEL: usize = NODES / PANELS;
const TEXT_STEP: usize = NODES / TEXTS;
const CACHE_CAPACITY: usize = 100_000;
const FRAMES: usize = 300;
const WARMUP: usize = 3;
const CACHE: SkiaCacheConfig = SkiaCacheConfig {
    text_capacity: CACHE_CAPACITY,
    geometry_capacity: CACHE_CAPACITY,
    geometry_bytes: CACHE_CAPACITY * 512,
};

fn cpu_backend() -> CpuRenderingBackend {
    CpuRenderingBackend {
        concurrency: fframes::get_thread_count(),
        text_cache_capacity: CACHE_CAPACITY,
        ..Default::default()
    }
}

fn circle_radius(slot: usize, frame: usize) -> f64 {
    if slot % 10 != 1 {
        return (16 + slot % 4 * 4) as f64;
    }
    let phase = ((slot + frame) % 60) as f64;
    let (progress, from, to) = if phase <= 30. {
        (phase / 30., 16., 28.)
    } else {
        ((phase - 30.) / 30., 28., 16.)
    };
    let amount = progress * progress * (3. - 2. * progress);
    amount * (to - from) + from
}

struct Grid;

impl Video for Grid {
    const FPS: usize = 30;
    const WIDTH: usize = VIDEO_WIDTH;
    const HEIGHT: usize = VIDEO_HEIGHT;
    const BACKGROUND_COLOR: Color = Color::hex("#18202c");
    fn duration(&self) -> Duration<'_> {
        Duration::Frames(FRAMES + WARMUP)
    }
    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }
    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let panels: Vec<_> = (0..PANELS)
            .map(|panel| {
                let circles: Vec<_> = (panel * PER_PANEL..(panel + 1) * PER_PANEL)
                    .filter(|slot| slot % TEXT_STEP != 0)
                    .map(|slot| {
                        let id = (slot + frame.index * 37) % NODES;
                        let radius = circle_radius(slot, frame.index);
                        let fill = Color::rgba(
                            ((id * 13 + frame.index * 17) % 256) as u8,
                            ((id * 7 + frame.index * 29) % 256) as u8,
                            ((id * 3 + frame.index * 43) % 256) as u8,
                            255,
                        );
                        fframes::svgr!(<circle cx={32 + (slot * 13 + frame.index * 3) % 128}
                        cy={32 + (slot * 17 + frame.index * 5) % 176}
                        r={radius} fill={fill} />)
                    })
                    .collect();
                let transform =
                    Transform::translate((panel % 5 * 200) as f64, (panel / 5 * 250) as f64);
                fframes::svgr!(<g transform={transform}>{circles}</g>)
            })
            .collect();
        let texts: Vec<_> = (0..NODES).step_by(TEXT_STEP).map(|slot| {
            let panel = slot / PER_PANEL;
            let index = slot % PER_PANEL / TEXT_STEP;
            let id = (slot + frame.index * 37) % NODES;
            fframes::svgr!(<text x={panel % 5 * 200 + 24 + index % 10 * 16}
                y={panel / 5 * 250 + 40 + index / 10 * 40}
                font-family="DM Sans" font-size="16" fill="#fff">{((id + frame.index) % 10).to_string()}</text>)
        }).collect();
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" width={VIDEO_WIDTH} height={VIDEO_HEIGHT}
                viewBox="0 0 1000 1000" preserveAspectRatio="none">
                {panels}
                {texts}
            </svg>
        )
    }
}

#[cfg(target_os = "macos")]
fn gpu_backend() -> Result<fframes_skia_renderer::metal::SkiaMetalCtx, Box<dyn std::error::Error>> {
    Ok(fframes_skia_renderer::metal::SkiaMetalCtx::new(
        VIDEO_WIDTH,
        VIDEO_HEIGHT,
    )?)
}

#[cfg(not(target_os = "macos"))]
fn gpu_backend() -> Result<fframes_skia_renderer::vulkan::SkiaVulkanCtx, Box<dyn std::error::Error>>
{
    use ash::vk;

    // Select hardware explicitly; lavapipe/llvmpipe is a CPU renderer.
    let entry = unsafe { ash::Entry::load()? };
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let instance = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default().application_info(&app),
            None,
        )?
    };
    let devices = unsafe { instance.enumerate_physical_devices()? };
    let device = devices.into_iter().find(|&device| {
        unsafe { instance.get_physical_device_properties(device) }.device_type
            != vk::PhysicalDeviceType::CPU
    });
    let Some(device) = device else {
        unsafe { instance.destroy_instance(None) };
        return Err("no hardware Vulkan GPU available".into());
    };
    Ok(
        fframes_skia_renderer::vulkan::SkiaVulkanCtx::new_with_device(
            entry,
            instance,
            device,
            VIDEO_WIDTH,
            VIDEO_HEIGHT,
        )?,
    )
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: vs-remotion-bench cpu|gpu OUTPUT_DIR");
    let out = Path::new(&args[2]);
    std::fs::create_dir_all(out).expect("output directory");
    match args[1].as_str() {
        "cpu" => run(cpu_backend, "cpu", out),
        "gpu" => match gpu_backend().and_then(|backend| {
            backend.create_skia_surface()?;
            Ok(backend)
        }) {
            Ok(backend) => run(
                || {
                    SkiaFFramesRenderer::new(
                        SkiaPipelineConfig {
                            concurrency_policy: SkiaPipelineConcurrencyPolicy::MaxPerformance,
                            cache: CACHE,
                            ..Default::default()
                        },
                        &backend,
                    )
                },
                if cfg!(target_os = "macos") {
                    "skia-metal"
                } else {
                    "skia-vulkan"
                },
                out,
            ),
            Err(error) => println!(
                "{}",
                serde_json::json!({"status": "skipped", "reason": error.to_string()})
            ),
        },
        _ => panic!("expected cpu or gpu"),
    }
}

fn run<B: FFramesRenderBackend>(pipeline: impl Fn() -> B, name: &str, out: &Path) {
    let video = Grid;
    let media = BenchMedia::prepare().expect("benchmark font");
    let hardware_encoding = name == "skia-metal";
    let encoder_name = if hardware_encoding {
        "h264_videotoolbox"
    } else {
        "libx264"
    };
    // Fail instead of silently benchmarking a different encoder.
    let codec_name = std::ffi::CString::new(encoder_name).expect("encoder name");
    assert!(
        !unsafe { fframes::ffmpeg_sys_fframes::avcodec_find_encoder_by_name(codec_name.as_ptr()) }
            .is_null(),
        "required encoder {encoder_name} unavailable"
    );
    let codec_params: &[(&str, &str)] = if hardware_encoding {
        &[("allow_sw", "0"), ("threads", "1")]
    } else {
        &[("preset", "medium"), ("threads", "1")]
    };
    let options = RenderOptions {
        media: Some(&media),
        load_system_fonts: false,
        logger: FFramesLoggerVariant::Silent,
        frame_range: Some(0..WARMUP),
        video_encoder_options: EncoderOptions {
            preferred_encoder: Some(encoder_name),
            codec_params: Some(codec_params),
            bitrate: Some(8_000_000),
            gop_size: 30,
            qmin: 0,
            qmax: 69,
            ..Default::default()
        },
        ..Default::default()
    };
    let warmup = out.join("warmup.mp4");
    fframes::render(&warmup, &video, pipeline(), &options).expect("warm-up MP4");
    let options = RenderOptions {
        frame_range: Some(WARMUP..WARMUP + FRAMES),
        ..options
    };
    let start = Instant::now();
    fframes::render(out.join("export.mp4"), &video, pipeline(), &options)
        .expect("complete MP4 export");
    let export_ms = start.elapsed().as_secs_f64() * 1000.;
    std::fs::remove_file(warmup).expect("remove warm-up MP4");
    let ffmpeg =
        unsafe { std::ffi::CStr::from_ptr(fframes::ffmpeg_sys_fframes::av_version_info()) }
            .to_string_lossy();
    let cache = if name == "cpu" {
        serde_json::json!({"text_capacity": cpu_backend().text_cache_capacity,
            "layer_capacity": cpu_backend().cache_capacity})
    } else {
        serde_json::json!({"text_capacity": CACHE.text_capacity,
            "geometry_capacity": CACHE.geometry_capacity, "geometry_bytes": CACHE.geometry_bytes})
    };
    let concurrency = if name == "cpu" {
        serde_json::json!({"render_threads": cpu_backend().concurrency})
    } else {
        serde_json::json!({"policy": "MaxPerformance",
            "encoder_threads": fframes::get_thread_count()})
    };
    println!(
        "{}",
        serde_json::json!({"backend": name, "nodes": NODES, "cache": cache,
            "concurrency": concurrency, "frames": FRAMES, "fps": Grid::FPS,
            "audio": false, "width": VIDEO_WIDTH, "height": VIDEO_HEIGHT,
            "export_ms": export_ms, "ffmpeg": ffmpeg,
            "encoder": encoder_name, "hardware_encoding": hardware_encoding})
    );
}
