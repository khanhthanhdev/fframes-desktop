//! Why it is fast, in one number: fframes against Remotion on the same 100,000 text
//! node video, then the three reasons.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::facts::*;
use crate::shaders::SHADERS;
use crate::ui::*;

short_scene!(FastScene, Some(super::FAST_AT), super::PARALLEL_AT);

pub const BLIP_BEATS: &[f32] = &[1.6, 2.6, 3.5, 4.4];

const LANE_X: f32 = 80.0;
const LANE_W: f32 = 920.0;

const REASONS: [&str; 3] = [
    "Skia draws on the GPU",
    "Static markup is cached",
    "Every core renders frames",
];

fn lane(y: f32, name: &str, time: String, width: f32, fill: &str, color: &str) -> Svgr<'static> {
    let name = name.to_owned();
    let fill = fill.to_owned();
    let color = color.to_owned();
    fframes::svgr!(
        <g>
            <text x={LANE_X} y={y} font-family={MONO} font-weight="600" font-size="24" letter-spacing="2" fill={color.clone()}>{name}</text>
            <text x={LANE_X + LANE_W} y={y} text-anchor="end" font-family={MONO} font-weight="600" font-size="28" fill={color}>{time}</text>
            <rect x={LANE_X} y={y + 18.0} width={LANE_W} height="40" fill={DIM} />
            <rect x={LANE_X} y={y + 18.0} width={width.max(0.5)} height="40" fill={fill} />
        </g>
    )
}

impl Scene for FastScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = SHADERS.grid.draw(
            &frame,
            ShaderUniforms::new()
                .float("uSpeed", 1.0)
                .float("uHorizon", 0.4)
                .float("uBright", 0.28)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let speedup = BENCH_REMOTION_S / BENCH_FFRAMES_S;
        // the number counts up, then settles
        let count = 1.0 + (speedup - 1.0) * expo_out(prog(lb, 0.1, 1.6));
        let big = Slam::new(80.0, 600.0, format!("{count:.1}×"), DISPLAY, 250.0, ACCENT);
        let tag = snap(lb - 0.2);

        // the race: both bars run on the same clock
        let e = prog(lb, 1.6, 4.4);
        let remotion = BENCH_REMOTION_S * e;
        let fframes_t = (BENCH_FFRAMES_S).min(remotion);
        let share = |t: f32| LANE_W * (t / BENCH_REMOTION_S);
        let race_o = prog(lb, 1.4, 1.7);

        let reasons: Vec<Svgr> = REASONS
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let at = 2.6 + i as f32 * 0.9;
                let s = snap(lb - at);
                let y = 1330.0 + i as f32 * 100.0;
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate(0 {})", (1.0 - s) * 40.0)}>
                        <rect x="80" y={y} width="920" height="80" fill="#0f0e0d" fill-opacity="0.92" stroke="#3d3a35" stroke-width="2" />
                        <rect x="80" y={y} width="8" height="80" fill={ACCENT} />
                        <text x="116" y={y + 52.0} font-family={DISPLAY} font-size="38" letter-spacing="-1" fill={BONE}>{*r}</text>
                    </g>
                )
            })
            .collect();

        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                <g opacity={prog(lb, 0.0, 0.3)} transform={format!("translate(0 {})", (1.0 - tag) * 24.0)}>
                    <text x="80" y="300" font-family={MONO} font-weight="600" font-size="24" letter-spacing="4" fill={ACCENT}>"WHY IT IS FAST"</text>
                    <text x="80" y="372" font-family={DISPLAY} font-size="56" letter-spacing="-1" fill={BONE}>"Same video. Same machine."</text>
                </g>
                {big.draw(lb - 0.2, -160.0, 0.0)}
                <g opacity={prog(lb, 0.6, 0.9)}>
                    <text x="84" y="690" font-family={DISPLAY} font-size="62" letter-spacing="-1" fill={BONE}>"FASTER THAN REMOTION"</text>
                </g>
                <g opacity={race_o}>
                    {lane(850.0, "REMOTION", format!("{remotion:.1} s"), share(remotion), "#76726c", BONE)}
                    {lane(970.0, "FFRAMES  /  SKIA GPU", format!("{fframes_t:.1} s"), share(fframes_t), ACCENT, ACCENT)}
                    <text x="80" y="1100" font-family={MONO} font-weight="500" font-size="21" fill={GREY}>"100,000 text nodes, 300 frames, 4K, Apple M-series"</text>
                    <text x="80" y="1134" font-family={MONO} font-weight="500" font-size="21" fill={GREY}>"Measured median render time."</text>
                </g>
                {reasons}
            </g>
        )
    }
}
