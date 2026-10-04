//! Copied into an ordinary portable Studio project by the integration fixture.
use fframes::{
    AudioMap, AudioTimestamp, AudioTrack, Color, Duration, FFramesContext, Frame, Overlap, Scene,
    Scenes, Shader, ShaderUniforms, Svgr, Video,
};
use std::sync::LazyLock;

/// Deliberately unsupported by the portable CPU preview. Keeping this static makes the
/// capability probe allocation-free after first use and preserves the scaffold's unit video.
static CAPABILITY_SHADER: LazyLock<Shader> = LazyLock::new(|| {
    Shader::sksl("half4 main(float2 p) { return half4(1, 0, 1, 1); }")
});

pub struct StudioVideo;

#[derive(Debug)]
struct Repeated;
impl Scene for Repeated {
    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(2.)
    }
    fn render_frame<'a>(&'a self, _frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(<text x="80" y="160" font-family="DM Sans" font-size="58" fill="#fff">"Repeated scene"</text>)
    }
}
#[derive(Debug)]
struct Overlay;
impl Scene for Overlay {
    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(2.)
    }
    fn overlap(&self) -> Overlap {
        Overlap::PreviousAndNext {
            previous: 0.5,
            next: 0.5,
        }
    }
    fn render_frame<'a>(&'a self, _frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(<text x="80" y="260" font-family="DM Sans" font-size="44" fill="#78b7fa">"Overlapping overlay"</text>)
    }
}
impl Video for StudioVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1280;
    const HEIGHT: usize = 720;
    const BACKGROUND_COLOR: Color = Color::TRANSPARENT;
    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }
    fn audio(&self) -> AudioMap<'_> {
        AudioMap::from([
            AudioTrack::new(
                "cue.wav",
                AudioTimestamp::Second(0.125)..AudioTimestamp::Second(1.125),
            )
            .gain_db(-3.0)
            .pan(-0.5)
            .fade_in(0.025)
            .fade_out(0.05)
            .offset(0.0625)
            .duck_under_voice(),
            AudioTrack::new(
                "cue.wav",
                AudioTimestamp::Second(3.25)..AudioTimestamp::Second(4.25),
            )
            .gain_db(-6.0)
            .pan(0.5)
            .fade_in(0.02)
            .fade_out(0.04)
            .voice(),
        ])
    }
    fn define_scenes(&self) -> Scenes<'_> {
        Scenes::from(vec![&Repeated as &dyn Scene, &Overlay, &Repeated])
    }
    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let x = (frame.index * 7 % 1100) + 80;
        let shader = CAPABILITY_SHADER.draw(&frame, ShaderUniforms::new());
        fframes::svgr!(<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 720" width={Self::WIDTH} height={Self::HEIGHT}>
            <rect width="1280" height="620" fill="#0d1117" />
            // Fixed, isolated centers are sampled by parity tests (RGBA, not inferred from text).
            <rect x="16" y="640" width="48" height="48" fill="#ff0000" />
            <rect x="80" y="640" width="48" height="48" fill="#00ff00" />
            <rect x="144" y="640" width="48" height="48" fill="#0000ff" fill-opacity="0.5019608" />
            <rect x="208" y="640" width="48" height="48" fill="#000000" fill-opacity="0" />
            {ctx.render_scenes(&frame)}
            <rect x={x} y="440" width="100" height="100" fill="#31be93" />
            <text x="80" y="360" font-family="DM Sans" font-size="40" fill="#fff">{format!("Frame {}", frame.index)}</text>
            <image href={shader.href()} x="1184" y="16" width="64" height="64" />
        </svg>)
    }
}
