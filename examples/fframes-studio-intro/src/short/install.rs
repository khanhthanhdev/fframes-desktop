//! Install, in one thumb-sized column: three rows that finish themselves.

use fframes::{Color, Duration, FFramesContext, Frame, Scene, ShaderUniforms, Svgr};

use crate::beat::*;
use crate::scenes::install::{bar, check};
use crate::shaders::SHADERS;
use crate::ui::*;

short_scene!(InstallScene, Some(0.0), 12.0);

/// Beats (on the video grid) where a step finishes.
pub const BLIP_BEATS: &[f32] = &[5.0, 7.6, 10.6];

struct Row {
    num: &'static str,
    title: &'static str,
    detail: &'static str,
    from: f32,
    to: f32,
}

const ROWS: [Row; 3] = [
    Row {
        num: "01",
        title: "Download the app",
        detail: "macOS  /  Windows  /  Linux",
        from: 2.0,
        to: 5.0,
    },
    Row {
        num: "02",
        title: "Connect an agent",
        detail: "Codex  /  Claude Code",
        from: 5.2,
        to: 7.6,
    },
    Row {
        num: "03",
        title: "Get the SDK",
        detail: "Pinned to the fframes source",
        from: 7.8,
        to: 10.6,
    },
];

const ROW_W: f32 = 920.0;
const ROW_H: f32 = 210.0;

impl Scene for InstallScene {
    fn duration(&self) -> Duration<'_> {
        Self::frames()
    }

    fn render_frame<'a>(&'a self, frame: Frame, _ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let lb = Self::lb(&frame);
        let bg = SHADERS.grid.draw(
            &frame,
            ShaderUniforms::new()
                .float("uSpeed", 0.5)
                .float("uHorizon", 0.4)
                .float("uBright", 0.22)
                .color("uInk", Color::hex("#6f6a63"))
                .color("uHot", Color::hex(ACCENT)),
        );
        let head = Slam::new(80.0, 440.0, "Install it.", DISPLAY, 116.0, BONE);
        let sub = Slam::new(80.0, 560.0, "like any app.", SERIF, 112.0, ACCENT).italic();
        let rows: Vec<Svgr> = ROWS
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let x = 80.0;
                let y = 700.0 + i as f32 * (ROW_H + 26.0);
                let at = 1.0 + i as f32 * 0.5;
                let s = snap(lb - at);
                let work = prog(lb, row.from, row.to);
                let done = work >= 1.0;
                let status = if done {
                    "READY"
                } else if work > 0.0 {
                    "WORKING"
                } else {
                    "WAITING"
                };
                let stroke = if done {
                    "#6fcf8a"
                } else if work > 0.0 {
                    ACCENT
                } else {
                    "#3d3a35"
                };
                fframes::svgr!(
                    <g opacity={prog(lb, at, at + 0.15)} transform={format!("translate(0 {})", (1.0 - s) * 60.0)}>
                        <rect x={x} y={y} width={ROW_W} height={ROW_H} fill="#0f0e0d" fill-opacity="0.92" stroke={stroke} stroke-width="2" />
                        <text x={x + 36.0} y={y + 54.0} font-family={MONO} font-weight="600" font-size="24" letter-spacing="3" fill={ACCENT}>{row.num}</text>
                        <text x={x + ROW_W - 36.0} y={y + 54.0} text-anchor="end" font-family={MONO} font-weight="500" font-size="20" letter-spacing="3" fill={if done { "#6fcf8a" } else { GREY }}>{status}</text>
                        <text x={x + 36.0} y={y + 118.0} font-family={DISPLAY} font-size="52" letter-spacing="-1" fill={BONE}>{row.title}</text>
                        <text x={x + 36.0} y={y + 158.0} font-family={MONO} font-weight="500" font-size="24" fill="#b9b4ab">{row.detail}</text>
                        {bar(x + 36.0, y + ROW_H - 26.0, ROW_W - 72.0, work)}
                        {check(x + ROW_W - 76.0, y + 92.0, prog(lb, row.to, row.to + 0.5))}
                    </g>
                )
            })
            .collect();
        let tag = snap(lb - 10.4);
        fframes::svgr!(
            <g>
                <image href={bg.href()} x="0" y="0" width="1080" height="1920" />
                {head.draw(lb, -120.0, 0.0)}
                {sub.draw(lb - 0.8, 120.0, 0.0)}
                {rows}
                <g opacity={prog(lb, 10.4, 10.7)} transform={format!("translate(0 {})", (1.0 - tag) * 24.0)}>
                    <text x="80" y="1560" font-family={MONO} font-weight="600" font-size="30" letter-spacing="3" fill={BONE}>"NO TERMINAL. NO SETUP SCRIPTS."</text>
                </g>
            </g>
        )
    }
}
