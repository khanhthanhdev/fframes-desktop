//! Which renderer a worker draws with. Studio passes `--renderer auto|gpu|skia-cpu|cpu`:
//! `auto` renders with Skia on the GPU whenever one works (Vulkan, Metal on macOS) and with
//! the built-in CPU renderer otherwise; Skia on the CPU runs only when it is asked for.
use fframes::{
    Color, CpuFrameRenderer, FFramesRendererError, FFramesRendererResult, FrameRenderer, RgbaFrame,
    usvgr,
};
use fframes_studio_protocol::{BackendChange, MAX_BACKEND_DETAIL_BYTES};

/// The built-in CPU renderer (tiny-skia). It cannot run shaders.
pub const CPU_BACKEND: &str = "cpu";
/// Skia on a GPU.
pub const SKIA_GPU_BACKEND: &str = "skia-gpu";
/// Skia on the CPU.
pub const SKIA_CPU_BACKEND: &str = "skia-cpu";
/// The capability a worker without Skia lacks.
pub const SHADER_PREVIEW_GAP: &str = "shader_preview";

/// The renderer a worker is asked to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RendererPreference {
    /// Skia on the GPU when one works, the built-in CPU renderer otherwise.
    #[default]
    Auto,
    /// Skia on the GPU, or an error naming why there is none.
    Gpu,
    /// Skia on the CPU: draws shaders, slower than the built-in renderer.
    SkiaCpu,
    /// The built-in CPU renderer.
    Cpu,
}

impl RendererPreference {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "gpu" => Some(Self::Gpu),
            "skia-cpu" => Some(Self::SkiaCpu),
            "cpu" => Some(Self::Cpu),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::SkiaCpu => "skia-cpu",
            Self::Cpu => "cpu",
        }
    }

    /// The value of `--renderer <value>` in `args`. Missing means [`Self::Auto`]; an unknown
    /// value is reported on stderr and also means [`Self::Auto`].
    pub fn from_args(args: &[String]) -> Self {
        let Some(value) = args
            .iter()
            .position(|arg| arg == "--renderer")
            .and_then(|index| args.get(index + 1))
        else {
            return Self::Auto;
        };
        Self::parse(value).unwrap_or_else(|| {
            eprintln!("unknown --renderer {value:?}, using auto");
            Self::Auto
        })
    }
}

/// The backend a worker renders with and what it runs on, or why the GPU was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendChoice {
    pub name: &'static str,
    pub detail: Option<String>,
}

impl BackendChoice {
    pub(crate) fn new(name: &'static str, detail: Option<String>) -> Self {
        Self {
            name,
            detail: detail.map(|detail| one_line(&detail)),
        }
    }

    /// `shader_preview` for the built-in CPU renderer, nothing for Skia.
    pub fn capability_gaps(&self) -> Vec<String> {
        if self.name == CPU_BACKEND {
            vec![SHADER_PREVIEW_GAP.into()]
        } else {
            Vec::new()
        }
    }
}

/// `text` on one line, at most [`MAX_BACKEND_DETAIL_BYTES`] long. Renderer errors span
/// several lines and carry terminal colors.
pub(crate) fn one_line(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // skip `ESC [ ... letter`
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    let mut line = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.len() > MAX_BACKEND_DETAIL_BYTES {
        let mut end = MAX_BACKEND_DETAIL_BYTES - '…'.len_utf8();
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

/// Makes a renderer of the selected backend; called once per output size.
pub(crate) type MakeRenderer<'a> = Box<dyn Fn() -> Box<dyn FrameRenderer + 'a> + 'a>;

/// How many output sizes keep their own renderer. Main frames and timeline thumbnails come
/// at different sizes; a Skia renderer drops its GPU context and caches on a size change.
pub(crate) const SIZE_CLASSES: usize = 2;

/// The renderer of a worker: Skia renderers per output size, or the built-in CPU renderer.
/// A Skia render that fails switches the rest of the session to the built-in renderer,
/// returns the error and reports the change once through [`Self::take_change`]. The caller
/// renders the frame again, which builds its tree with the exact path geometry the built-in
/// renderer is tested against.
pub(crate) struct Renderers<'a> {
    choice: BackendChoice,
    make: Option<MakeRenderer<'a>>,
    /// Most recently used last.
    sized: Vec<((u32, u32), Box<dyn FrameRenderer + 'a>)>,
    cpu: CpuFrameRenderer,
    change: Option<BackendChange>,
}

impl<'a> Renderers<'a> {
    pub(crate) fn cpu(choice: BackendChoice) -> Self {
        Self {
            choice,
            make: None,
            sized: Vec::new(),
            cpu: CpuFrameRenderer::default(),
            change: None,
        }
    }

    #[cfg_attr(not(feature = "skia"), allow(dead_code))]
    pub(crate) fn skia(choice: BackendChoice, make: MakeRenderer<'a>) -> Self {
        Self {
            make: Some(make),
            ..Self::cpu(choice)
        }
    }

    pub(crate) fn choice(&self) -> &BackendChoice {
        &self.choice
    }

    /// The backend change of the last frame, if it switched.
    pub(crate) fn take_change(&mut self) -> Option<BackendChange> {
        self.change.take()
    }

    fn sized_renderer(&mut self, size: (u32, u32)) -> Option<&mut Box<dyn FrameRenderer + 'a>> {
        let make = self.make.as_ref()?;
        match self.sized.iter().position(|(s, _)| *s == size) {
            Some(index) => {
                let entry = self.sized.remove(index);
                self.sized.push(entry);
            }
            None => {
                if self.sized.len() == SIZE_CLASSES {
                    self.sized.remove(0);
                }
                self.sized.push((size, make()));
            }
        }
        self.sized.last_mut().map(|(_, renderer)| renderer)
    }

    fn fall_back(&mut self, error: &FFramesRendererError) {
        let detail = format!(
            "{} failed, using the CPU renderer: {error}",
            self.choice.name
        );
        eprintln!("{}", one_line(&detail));
        self.sized.clear();
        self.make = None;
        self.choice = BackendChoice::new(CPU_BACKEND, Some(detail));
        self.change = Some(BackendChange {
            backend: self.choice.name.into(),
            capability_gaps: self.choice.capability_gaps(),
            backend_detail: self.choice.detail.clone(),
        });
    }
}

impl FrameRenderer for Renderers<'_> {
    fn fast_shapes(&self) -> bool {
        // Skia draws ovals and rounded rects analytically
        self.make.is_some() || self.cpu.fast_shapes()
    }

    fn svg_text_cache_capacity(&self) -> Option<usize> {
        match self.sized.last() {
            Some((_, renderer)) => renderer.svg_text_cache_capacity(),
            None => self.cpu.svg_text_cache_capacity(),
        }
    }

    fn render_tree(
        &mut self,
        tree: &usvgr::Tree,
        background: Color,
        width: u32,
        height: u32,
    ) -> FFramesRendererResult<RgbaFrame> {
        if let Some(renderer) = self.sized_renderer((width, height)) {
            // Errors of user content surface while the tree is built, before this call, so
            // whatever fails here is the backend.
            let result = renderer.render_tree(tree, background, width, height);
            if let Err(error) = &result {
                self.fall_back(error);
            }
            return result;
        }
        self.cpu.render_tree(tree, background, width, height)
    }
}

#[cfg(feature = "skia")]
pub(crate) use skia::*;

#[cfg(feature = "skia")]
mod skia {
    use super::*;
    use fframes::{
        AudioMap, Duration, EncoderInput, FFramesContext, Frame, Previewer, RenderOptions, Shader,
        ShaderUniforms, Svgr, Video, VideoEncoderInfo,
    };
    use fframes_skia_renderer::skia_safe::{Surface, gpu::DirectContext};
    use fframes_skia_renderer::{
        HardwareFrameTarget, SkiaBackend, SkiaContext, SkiaCpuCtx, SkiaFrameRenderer,
    };

    /// A Skia context of one of the backends a worker can use.
    pub enum SkiaCtx {
        Cpu(SkiaCpuCtx),
        #[cfg(not(target_os = "macos"))]
        Vulkan(Box<fframes_skia_renderer::vulkan::SkiaVulkanCtx>),
        #[cfg(target_os = "macos")]
        Metal(fframes_skia_renderer::metal::SkiaMetalCtx),
    }

    impl SkiaCtx {
        fn backend(&self) -> &dyn SkiaBackend {
            match self {
                Self::Cpu(ctx) => ctx,
                #[cfg(not(target_os = "macos"))]
                Self::Vulkan(ctx) => ctx.as_ref(),
                #[cfg(target_os = "macos")]
                Self::Metal(ctx) => ctx,
            }
        }
    }

    impl SkiaBackend for SkiaCtx {
        fn create_skia_surface(&self) -> FFramesRendererResult<(Surface, Option<DirectContext>)> {
            self.backend().create_skia_surface()
        }

        fn create_skia_context(&self) -> FFramesRendererResult<SkiaContext> {
            self.backend().create_skia_context()
        }

        fn negotiate_hardware_frames(
            &self,
            encoder: &VideoEncoderInfo<'_>,
        ) -> Option<EncoderInput> {
            self.backend().negotiate_hardware_frames(encoder)
        }

        fn hardware_frame_target(
            &self,
            input: &EncoderInput,
            width: i32,
            height: i32,
        ) -> FFramesRendererResult<(DirectContext, Box<dyn HardwareFrameTarget>)> {
            self.backend().hardware_frame_target(input, width, height)
        }
    }

    /// The GPU context of this platform at `width`x`height` and what it runs on: Metal on
    /// macOS, Vulkan elsewhere. A Vulkan device that is a software rasterizer counts as no GPU.
    pub fn platform_gpu(width: usize, height: usize) -> FFramesRendererResult<(SkiaCtx, String)> {
        #[cfg(target_os = "macos")]
        {
            let ctx = fframes_skia_renderer::metal::SkiaMetalCtx::new(width, height)?;
            Ok((SkiaCtx::Metal(ctx), "metal".into()))
        }
        #[cfg(not(target_os = "macos"))]
        {
            use fframes_skia_renderer::vulkan::ash::vk::PhysicalDeviceType;
            let ctx = fframes_skia_renderer::vulkan::SkiaVulkanCtx::new(width, height)?;
            let (name, kind) = ctx.device_info();
            let kind = match kind {
                PhysicalDeviceType::DISCRETE_GPU => "discrete",
                PhysicalDeviceType::INTEGRATED_GPU => "integrated",
                PhysicalDeviceType::VIRTUAL_GPU => "virtual",
                PhysicalDeviceType::CPU => {
                    return Err(FFramesRendererError::Skia(format!(
                        "the only Vulkan device, {name}, renders on the CPU"
                    )));
                }
                _ => "other",
            };
            Ok((
                SkiaCtx::Vulkan(Box::new(ctx)),
                format!("vulkan {name} ({kind})"),
            ))
        }
    }

    /// The Skia context a worker selected and how it is reported. `ctx` is `None` for the
    /// built-in CPU renderer.
    pub struct Selection {
        pub ctx: Option<SkiaCtx>,
        pub choice: BackendChoice,
    }

    impl Selection {
        /// The renderers of the selection, borrowing its context.
        pub(crate) fn renderers(&self) -> Renderers<'_> {
            match &self.ctx {
                Some(ctx) => Renderers::skia(
                    self.choice.clone(),
                    Box::new(move || Box::new(SkiaFrameRenderer::new(ctx))),
                ),
                None => Renderers::cpu(self.choice.clone()),
            }
        }
    }

    /// Selects the backend for `preference` at `width`x`height`. `gpu` opens the GPU context
    /// (`platform_gpu` outside tests). Skia contexts must draw [`probe`] correctly to be used.
    pub fn select(
        preference: RendererPreference,
        width: usize,
        height: usize,
        gpu: impl FnOnce(usize, usize) -> FFramesRendererResult<(SkiaCtx, String)>,
    ) -> FFramesRendererResult<Selection> {
        let cpu = |reason: String| Selection {
            ctx: None,
            choice: BackendChoice::new(CPU_BACKEND, Some(reason)),
        };
        match preference {
            RendererPreference::Cpu => Ok(Selection {
                ctx: None,
                choice: BackendChoice::new(CPU_BACKEND, None),
            }),
            RendererPreference::SkiaCpu => {
                let ctx = SkiaCtx::Cpu(SkiaCpuCtx::new(width, height));
                match probe(&ctx) {
                    Ok(()) => Ok(Selection {
                        ctx: Some(ctx),
                        choice: BackendChoice::new(SKIA_CPU_BACKEND, None),
                    }),
                    Err(error) => Ok(cpu(format!("Skia on the CPU failed: {error}"))),
                }
            }
            RendererPreference::Gpu | RendererPreference::Auto => {
                let opened = gpu(width, height).and_then(|(ctx, detail)| {
                    probe(&ctx)?;
                    Ok((ctx, detail))
                });
                match opened {
                    Ok((ctx, detail)) => Ok(Selection {
                        ctx: Some(ctx),
                        choice: BackendChoice::new(SKIA_GPU_BACKEND, Some(detail)),
                    }),
                    Err(error) if preference == RendererPreference::Gpu => Err(
                        FFramesRendererError::Skia(one_line(&format!("no usable GPU: {error}"))),
                    ),
                    Err(error) => Ok(cpu(format!("no usable GPU: {error}"))),
                }
            }
        }
    }

    /// The color the probe shader fills its half with.
    const PROBE_SHADER: [u8; 3] = [0, 255, 0];
    /// The color of the plain rectangle next to it.
    const PROBE_RECT: [u8; 3] = [255, 0, 0];

    struct ProbeVideo {
        shader: Shader,
    }

    impl Video for ProbeVideo {
        const FPS: usize = 1;
        const WIDTH: usize = 64;
        const HEIGHT: usize = 64;

        fn duration(&self) -> Duration<'_> {
            Duration::Frames(1)
        }

        fn audio(&self) -> AudioMap<'_> {
            AudioMap::none()
        }

        fn render_frame<'a>(&'a self, frame: Frame, _: &FFramesContext<'a, '_>) -> Svgr<'a> {
            let layer = self.shader.draw(&frame, ShaderUniforms::new());
            fframes::svgr!(
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
                    <image href={layer.href()} x="0" y="0" width="32" height="64" />
                    <rect x="32" y="0" width="32" height="64" fill="#ff0000" />
                </svg>
            )
        }
    }

    /// Draws a 64x64 frame with a shader on the left and a rectangle on the right with a
    /// renderer of `ctx` and checks both halves. Never renders user content, so a failure
    /// is the backend's.
    pub fn probe(ctx: &SkiaCtx) -> FFramesRendererResult<()> {
        let video = ProbeVideo {
            shader: Shader::sksl("half4 main(float2 coord) { return half4(0, 1, 0, 1); }"),
        };
        let mut previewer = Previewer::new(&video, &RenderOptions::default())?;
        let mut renderer = SkiaFrameRenderer::new(ctx);
        let frame = previewer.render(0, &mut renderer)?;
        let pixel = |x: usize, y: usize| {
            let at = (y * frame.width as usize + x) * 4;
            [frame.pixels[at], frame.pixels[at + 1], frame.pixels[at + 2]]
        };
        let close = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 8);
        let (shader, rect) = (pixel(16, 32), pixel(48, 32));
        if !close(shader, PROBE_SHADER) || !close(rect, PROBE_RECT) {
            return Err(FFramesRendererError::Skia(format!(
                "probe frame drew {shader:?} and {rect:?} instead of {PROBE_SHADER:?} and {PROBE_RECT:?}"
            )));
        }
        Ok(())
    }
}

/// The selection when the crate is built without Skia: always the built-in renderer.
#[cfg(not(feature = "skia"))]
pub(crate) fn cpu_only(preference: RendererPreference) -> FFramesRendererResult<BackendChoice> {
    match preference {
        RendererPreference::Gpu | RendererPreference::SkiaCpu => {
            Err(FFramesRendererError::Custom(format!(
                "--renderer {} needs a worker built with the `skia` feature",
                preference.as_str()
            )))
        }
        _ => Ok(BackendChoice::new(CPU_BACKEND, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_arguments_parse_and_default_to_auto() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            RendererPreference::from_args(&args(&["--x", "--renderer", "skia-cpu"])),
            RendererPreference::SkiaCpu
        );
        assert_eq!(
            RendererPreference::from_args(&args(&["--renderer", "cpu"])),
            RendererPreference::Cpu
        );
        assert_eq!(
            RendererPreference::from_args(&args(&["--renderer", "metal"])),
            RendererPreference::Auto
        );
        assert_eq!(
            RendererPreference::from_args(&args(&["--renderer"])),
            RendererPreference::Auto
        );
        assert_eq!(RendererPreference::from_args(&[]), RendererPreference::Auto);
    }

    #[test]
    fn details_are_one_bounded_line_without_colors() {
        let detail = one_line("\u{1b}[31mFailure\u{1b}[0m\nSkia error:   lost\n");
        assert_eq!(detail, "Failure Skia error: lost");
        let long = one_line(&"é".repeat(400));
        assert!(long.len() <= MAX_BACKEND_DETAIL_BYTES);
        assert!(long.ends_with('…'));
    }

    /// A renderer that fails after `ok` frames and counts how often it was made.
    struct Flaky {
        ok: usize,
    }

    impl FrameRenderer for Flaky {
        fn fast_shapes(&self) -> bool {
            true
        }
        fn svg_text_cache_capacity(&self) -> Option<usize> {
            None
        }
        fn render_tree(
            &mut self,
            _: &usvgr::Tree,
            _: Color,
            width: u32,
            height: u32,
        ) -> FFramesRendererResult<RgbaFrame> {
            if self.ok == 0 {
                return Err(FFramesRendererError::Skia("device lost".into()));
            }
            self.ok -= 1;
            Ok(RgbaFrame {
                width,
                height,
                pixels: vec![7; (width * height * 4) as usize],
            })
        }
    }

    fn tree() -> usvgr::Tree {
        usvgr::Tree::from_str(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#00f"/></svg>"##,
            &usvgr::Options::default(),
            &Default::default(),
        )
        .unwrap()
    }

    #[test]
    fn alternating_sizes_keep_one_renderer_each() {
        let made = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = made.clone();
        let mut renderers = Renderers::skia(
            BackendChoice::new(SKIA_GPU_BACKEND, None),
            Box::new(move || {
                counter.set(counter.get() + 1);
                Box::new(Flaky { ok: usize::MAX })
            }),
        );
        let tree = tree();
        for size in [(8, 8), (2, 2), (8, 8), (2, 2), (8, 8)] {
            renderers
                .render_tree(&tree, Color::BLACK, size.0, size.1)
                .unwrap();
        }
        assert_eq!(
            made.get(),
            2,
            "main and thumbnail sizes keep their renderers"
        );
        // a third size evicts the least recently used one (2x2)
        renderers.render_tree(&tree, Color::BLACK, 4, 4).unwrap();
        renderers.render_tree(&tree, Color::BLACK, 8, 8).unwrap();
        assert_eq!(made.get(), 3);
        renderers.render_tree(&tree, Color::BLACK, 2, 2).unwrap();
        assert_eq!(made.get(), 4);
    }

    #[test]
    fn a_failing_gpu_switches_the_session_to_the_cpu_renderer_once() {
        let mut renderers = Renderers::skia(
            BackendChoice::new(SKIA_GPU_BACKEND, Some("vulkan test".into())),
            Box::new(|| Box::new(Flaky { ok: 1 })),
        );
        let tree = tree();
        let first = renderers.render_tree(&tree, Color::BLACK, 4, 4).unwrap();
        assert_eq!(first.pixels[0], 7, "the GPU renderer drew the first frame");
        assert!(renderers.take_change().is_none());

        assert!(renderers.render_tree(&tree, Color::BLACK, 4, 4).is_err());
        let change = renderers.take_change().expect("the switch is reported");
        assert!(
            !renderers.fast_shapes(),
            "the frame is rebuilt for the CPU renderer"
        );
        let again = renderers.render_tree(&tree, Color::BLACK, 4, 4).unwrap();
        assert_eq!(
            &again.pixels[..3],
            &[0, 0, 255],
            "the CPU renderer draws the retry"
        );
        assert_eq!(change.backend, CPU_BACKEND);
        assert_eq!(change.capability_gaps, vec![SHADER_PREVIEW_GAP.to_string()]);
        assert!(change.backend_detail.unwrap().contains("device lost"));
        assert_eq!(renderers.choice().name, CPU_BACKEND);

        renderers.render_tree(&tree, Color::BLACK, 4, 4).unwrap();
        assert!(renderers.take_change().is_none(), "reported once");
    }

    /// Static circles and rounded rects, the shapes a Skia tree keeps as fast shapes.
    struct ShapesVideo;

    impl fframes::Video for ShapesVideo {
        const FPS: usize = 1;
        const WIDTH: usize = 64;
        const HEIGHT: usize = 64;

        fn duration(&self) -> fframes::Duration<'_> {
            fframes::Duration::Frames(1)
        }

        fn audio(&self) -> fframes::AudioMap<'_> {
            fframes::AudioMap::none()
        }

        fn render_frame<'a>(
            &'a self,
            _: fframes::Frame,
            _: &fframes::FFramesContext<'a, '_>,
        ) -> fframes::Svgr<'a> {
            fframes::svgr!(
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64">
                    <rect x="4" y="4" width="56" height="24" rx="8" fill="#ff0000" />
                    <circle cx="32" cy="46" r="14" fill="#00ff00" />
                </svg>
            )
        }
    }

    #[test]
    fn the_frame_rebuilt_after_a_gpu_failure_draws_like_the_cpu_renderer() {
        let options = fframes::RenderOptions::default();
        let mut previewer = fframes::Previewer::new(&ShapesVideo, &options).unwrap();
        let expected = previewer
            .render(0, &mut CpuFrameRenderer::default())
            .unwrap();
        assert_eq!(&expected.pixels[(46 * 64 + 32) * 4..][..3], &[0, 255, 0]);

        let mut renderers = Renderers::skia(
            BackendChoice::new(SKIA_GPU_BACKEND, None),
            Box::new(|| Box::new(Flaky { ok: 0 })),
        );
        assert!(previewer.render(0, &mut renderers).is_err());
        assert!(renderers.take_change().is_some());
        let rebuilt = previewer.render(0, &mut renderers).unwrap();
        assert!(
            rebuilt.pixels == expected.pixels,
            "the retry differs from a CPU render"
        );
    }

    #[cfg(feature = "skia")]
    mod skia {
        use super::super::*;
        use fframes_skia_renderer::SkiaCpuCtx;

        fn fake_gpu(width: usize, height: usize) -> FFramesRendererResult<(SkiaCtx, String)> {
            Ok((
                SkiaCtx::Cpu(SkiaCpuCtx::new(width, height)),
                "test gpu".into(),
            ))
        }

        fn no_gpu(_: usize, _: usize) -> FFramesRendererResult<(SkiaCtx, String)> {
            Err(FFramesRendererError::Skia(
                "Failed to load Vulkan entry".into(),
            ))
        }

        #[test]
        fn auto_uses_a_working_gpu() {
            let selection = select(RendererPreference::Auto, 64, 64, fake_gpu).unwrap();
            assert_eq!(selection.choice.name, SKIA_GPU_BACKEND);
            assert_eq!(selection.choice.detail.as_deref(), Some("test gpu"));
            assert!(selection.choice.capability_gaps().is_empty());
        }

        #[test]
        fn auto_without_a_gpu_uses_the_cpu_renderer_and_says_why() {
            let selection = select(RendererPreference::Auto, 64, 64, no_gpu).unwrap();
            assert!(selection.ctx.is_none());
            assert_eq!(selection.choice.name, CPU_BACKEND);
            assert_eq!(selection.choice.capability_gaps(), vec![SHADER_PREVIEW_GAP]);
            assert!(
                selection
                    .choice
                    .detail
                    .unwrap()
                    .contains("Failed to load Vulkan entry")
            );
        }

        #[test]
        fn a_forced_gpu_without_one_is_an_error() {
            let error = select(RendererPreference::Gpu, 64, 64, no_gpu)
                .err()
                .expect("no fallback for --renderer gpu");
            assert!(error.to_string().contains("no usable GPU"));
        }

        #[test]
        fn skia_cpu_is_used_only_when_asked_for() {
            let selection = select(RendererPreference::SkiaCpu, 64, 64, no_gpu).unwrap();
            assert_eq!(selection.choice.name, SKIA_CPU_BACKEND);
            let selection = select(RendererPreference::Cpu, 64, 64, fake_gpu).unwrap();
            assert_eq!(selection.choice.name, CPU_BACKEND);
            assert!(selection.choice.detail.is_none());
        }

        #[test]
        fn the_probe_tells_skia_from_the_cpu_renderer() {
            probe(&SkiaCtx::Cpu(SkiaCpuCtx::new(64, 64))).expect("Skia draws the probe shader");
        }
    }
}
